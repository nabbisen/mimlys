//! The snapshot files (external design §14.1, §14.3; REQ-WS-004,
//! REQ-PERS-009): objects once by content hash, manifests by snapshot id,
//! and the journal that makes a snapshot a transaction the next start
//! finishes or undoes, so that no orphan ever needs collecting (RFC 006 §6).
//!
//! Durability order of one snapshot: journal (fsynced) → objects (each
//! written to a temporary name, fsynced, renamed, read-only) → manifest
//! (fsynced) → the database row → the journal removed.

use crate::sqlite::{create_private_dir, io, restrict_file};
use mimlys_core::hash::{Hash, hash_bytes};
use mimlys_core::ids::SnapshotId;
use mimlys_core::snapshot::Manifest;
use mimlys_core::spi::PersistenceError;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The snapshot directories under the data directory.
pub struct SnapshotFiles {
    root: PathBuf,
}

impl SnapshotFiles {
    /// `<data_dir>/snapshots/{objects,manifests,pending}`, created owner-only.
    pub fn open(data_dir: &Path) -> Result<Self, PersistenceError> {
        let root = data_dir.join("snapshots");
        for sub in ["objects", "manifests", "pending"] {
            create_private_dir(&root.join(sub))?;
        }
        Ok(Self { root })
    }

    /// The snapshot directories for reading only: nothing is created, locked or
    /// recovered (REQ-PERS-013). A directory that is not there reads as empty.
    pub fn open_for_reading(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("snapshots"),
        }
    }

    fn object_path(&self, hash: &Hash) -> PathBuf {
        self.root.join("objects").join(hash.as_str())
    }

    fn manifest_path(&self, snapshot_id: SnapshotId) -> PathBuf {
        self.root
            .join("manifests")
            .join(format!("{snapshot_id}.json"))
    }

    fn journal_path(&self, snapshot_id: SnapshotId) -> PathBuf {
        self.root.join("pending").join(snapshot_id.to_string())
    }

    /// Whether the object exists.
    pub fn has_object(&self, hash: &Hash) -> bool {
        self.object_path(hash).is_file()
    }

    /// Writes an object from a reader, verifying the hash the caller computed;
    /// a present object is left alone (REQ-WS-004).
    pub fn put_object(&self, hash: &Hash, mut content: impl Read) -> Result<(), PersistenceError> {
        let path = self.object_path(hash);
        if path.is_file() {
            return Ok(());
        }
        let tmp =
            self.root
                .join("objects")
                .join(format!("{}.tmp-{}", hash.as_str(), std::process::id()));
        let mut file = std::fs::File::create(&tmp).map_err(io("object_create"))?;
        let mut hasher = sha2::Sha256::default();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = content.read(&mut buf).map_err(io("object_read"))?;
            if n == 0 {
                break;
            }
            sha2::Digest::update(&mut hasher, &buf[..n]);
            file.write_all(&buf[..n]).map_err(io("object_write"))?;
        }
        let actual: String = sha2::Digest::finalize(hasher)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != hash.as_str() {
            let _ = std::fs::remove_file(&tmp);
            return Err(PersistenceError::Storage {
                detail: "object_hash_mismatch".into(),
            });
        }
        file.sync_all().map_err(io("object_sync"))?;
        drop(file);
        std::fs::rename(&tmp, &path).map_err(io("object_rename"))?;
        make_read_only(&path);
        sync_dir(&self.root.join("objects"))?;
        Ok(())
    }

    /// Opens an object for reading; the caller verifies the hash as it copies.
    pub fn open_object(&self, hash: &Hash) -> Result<std::fs::File, PersistenceError> {
        std::fs::File::open(self.object_path(hash)).map_err(io("object_open"))
    }

    /// Removes an object whose reference count reached zero.
    pub fn remove_object(&self, hash: &Hash) -> Result<(), PersistenceError> {
        let path = self.object_path(hash);
        if !path.exists() {
            return Ok(());
        }
        make_writable(&path);
        std::fs::remove_file(&path).map_err(io("object_remove"))
    }

    /// Writes a manifest, fsynced; returns its hash.
    pub fn put_manifest(
        &self,
        snapshot_id: SnapshotId,
        manifest: &Manifest,
    ) -> Result<Hash, PersistenceError> {
        let bytes = manifest.canonical_bytes();
        let hash = hash_bytes(&bytes);
        let path = self.manifest_path(snapshot_id);
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp).map_err(io("manifest_create"))?;
        file.write_all(&bytes).map_err(io("manifest_write"))?;
        file.sync_all().map_err(io("manifest_sync"))?;
        drop(file);
        std::fs::rename(&tmp, &path).map_err(io("manifest_rename"))?;
        restrict_file(&path)?;
        sync_dir(&self.root.join("manifests"))?;
        Ok(hash)
    }

    /// Reads a manifest and checks it against the recorded hash (REQ-PERS-009).
    pub fn read_manifest(
        &self,
        snapshot_id: SnapshotId,
        expected: &Hash,
    ) -> Result<Manifest, PersistenceError> {
        let bytes = std::fs::read(self.manifest_path(snapshot_id)).map_err(io("manifest_read"))?;
        if hash_bytes(&bytes) != *expected {
            return Err(PersistenceError::Storage {
                detail: "manifest_hash".into(),
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| PersistenceError::Storage {
            detail: "manifest_parse".into(),
        })
    }

    /// Whether the manifest file exists and matches the hash.
    pub fn manifest_ok(&self, snapshot_id: SnapshotId, expected: &Hash) -> bool {
        self.read_manifest(snapshot_id, expected).is_ok()
    }

    /// Removes a manifest file.
    pub fn remove_manifest(&self, snapshot_id: SnapshotId) -> Result<(), PersistenceError> {
        let path = self.manifest_path(snapshot_id);
        if path.exists() {
            std::fs::remove_file(&path).map_err(io("manifest_remove"))?;
        }
        Ok(())
    }

    /// Opens the journal of a snapshot about to be written: the objects it
    /// will add, fsynced before the first object (RFC 006 §6).
    pub fn begin(
        &self,
        snapshot_id: SnapshotId,
        new_objects: &[Hash],
    ) -> Result<(), PersistenceError> {
        let path = self.journal_path(snapshot_id);
        let text = new_objects
            .iter()
            .map(|h| h.as_str().to_owned())
            .collect::<Vec<_>>()
            .join("\n");
        let mut file = std::fs::File::create(&path).map_err(io("journal_create"))?;
        file.write_all(text.as_bytes())
            .map_err(io("journal_write"))?;
        file.sync_all().map_err(io("journal_sync"))?;
        sync_dir(&self.root.join("pending"))?;
        Ok(())
    }

    /// Closes the journal once the row is durable.
    pub fn finish(&self, snapshot_id: SnapshotId) -> Result<(), PersistenceError> {
        let path = self.journal_path(snapshot_id);
        if path.exists() {
            std::fs::remove_file(&path).map_err(io("journal_remove"))?;
        }
        Ok(())
    }

    /// Undoes a journal without a row: its new objects and its manifest go.
    pub fn undo(&self, snapshot_id: SnapshotId) -> Result<usize, PersistenceError> {
        let path = self.journal_path(snapshot_id);
        let text = std::fs::read_to_string(&path).map_err(io("journal_read"))?;
        let mut removed = 0;
        for line in text.lines().filter(|l| !l.is_empty()) {
            if let Some(hash) = Hash::from_hex(line) {
                self.remove_object(&hash)?;
                removed += 1;
            }
        }
        self.remove_manifest(snapshot_id)?;
        for entry in std::fs::read_dir(self.root.join("objects"))
            .map_err(io("objects_dir"))?
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".tmp-") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        std::fs::remove_file(&path).map_err(io("journal_remove"))?;
        Ok(removed)
    }

    /// The pending journals, as snapshot ids.
    pub fn pending(&self) -> Result<Vec<SnapshotId>, PersistenceError> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(self.root.join("pending"))
            .map_err(io("pending_dir"))?
            .flatten()
        {
            if let Ok(uuid) = uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()) {
                out.push(SnapshotId::from_uuid(uuid));
            }
        }
        Ok(out)
    }
}

/// What the startup step did with the pending journals (RFC 006 §6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JournalReport {
    /// Journals whose row existed: removed.
    pub finished: usize,
    /// Journals without a row: their objects and manifest removed.
    pub undone: usize,
    /// Objects removed by the undone journals.
    pub objects_removed: usize,
}

fn make_read_only(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_readonly(true);
        let _ = std::fs::set_permissions(path, perms);
    }
}

fn make_writable(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
    }
}

fn sync_dir(path: &Path) -> Result<(), PersistenceError> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)
            .and_then(|d| d.sync_all())
            .map_err(io("dir_sync"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
