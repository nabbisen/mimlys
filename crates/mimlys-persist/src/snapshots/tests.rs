use super::*;
use mimlys_core::ids::ScriptedIds;
use mimlys_core::snapshot::Entry;

fn files() -> (tempfile::TempDir, SnapshotFiles) {
    let dir = tempfile::tempdir().unwrap();
    let files = SnapshotFiles::open(dir.path()).unwrap();
    (dir, files)
}

/// REQ-WS-004 / REQ-PERS-009: objects are written once by hash, verified,
/// read-only; manifests are checked against their hash when read.
#[test]
fn req_ws_004_objects_and_manifests() {
    let (_dir, f) = files();
    let hash = hash_bytes(b"hello");
    f.put_object(&hash, &b"hello"[..]).unwrap();
    assert!(f.has_object(&hash));
    let meta = std::fs::metadata(f.object_path(&hash)).unwrap();
    assert!(meta.permissions().readonly());
    f.put_object(&hash, &b"hello"[..]).unwrap();
    assert!(matches!(
        f.put_object(&hash_bytes(b"other"), &b"hello"[..]),
        Err(PersistenceError::Storage { detail }) if detail == "object_hash_mismatch"
    ));
    assert!(!f.has_object(&hash_bytes(b"other")));
    let mut out = String::new();
    f.open_object(&hash)
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert_eq!(out, "hello");
    let ids = ScriptedIds::default();
    let id = SnapshotId::new(&ids);
    let manifest = Manifest::new(vec![Entry::File {
        path: "a".into(),
        executable: false,
        size: 5,
        hash: hash.clone(),
    }]);
    let mhash = f.put_manifest(id, &manifest).unwrap();
    assert_eq!(mhash, manifest.hash());
    assert_eq!(f.read_manifest(id, &mhash).unwrap(), manifest);
    assert!(
        matches!(f.read_manifest(id, &hash), Err(PersistenceError::Storage { detail }) if detail == "manifest_hash")
    );
    f.remove_object(&hash).unwrap();
    assert!(!f.has_object(&hash));
}

/// RFC 006 §6: a journal without a row is undone (its new objects and manifest
/// removed); one with a row is finished; nothing else is touched.
#[test]
fn rfc_006_journal_finish_or_undo() {
    let (_dir, f) = files();
    let ids = ScriptedIds::default();
    let kept = hash_bytes(b"kept");
    f.put_object(&kept, &b"kept"[..]).unwrap();
    let crashed = SnapshotId::new(&ids);
    let new = hash_bytes(b"new");
    f.begin(crashed, std::slice::from_ref(&new)).unwrap();
    f.put_object(&new, &b"new"[..]).unwrap();
    f.put_manifest(crashed, &Manifest::default()).unwrap();
    // A crash here: no row. The next start undoes the journal.
    assert_eq!(f.pending().unwrap(), [crashed]);
    assert_eq!(f.undo(crashed, &|_| Ok(false)).unwrap(), 1);
    assert!(!f.has_object(&new));
    assert!(
        f.has_object(&kept),
        "an object the journal did not add stays"
    );
    assert!(!f.manifest_path(crashed).exists());
    assert!(f.pending().unwrap().is_empty());
    let done = SnapshotId::new(&ids);
    f.begin(done, &[]).unwrap();
    f.finish(done).unwrap();
    assert!(f.pending().unwrap().is_empty());
}
