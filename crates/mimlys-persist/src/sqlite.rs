//! Opening the store (REQ-PERS-001, REQ-PERS-002, REQ-SEC-008, REQ-WS-001):
//! data directory and files owner-only, the lock file, WAL with synchronous
//! FULL, foreign keys, a busy timeout of LIM-O3, forward-only migrations,
//! and a refusal of a newer schema.

use mimlys_core::spi::PersistenceError;
use rusqlite::Connection;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The schema version this runtime writes and understands.
pub const SCHEMA_VERSION: u32 = 2;

/// LIM-O3: wait for the runtime lock before refusing to start, in milliseconds.
pub const LIM_O3_LOCK_WAIT_MS: u64 = 1000;

/// The file beside the lock in which the writer records who holds it
/// (REQ-PERS-013): `{"pid": N, "since": "<RFC 3339>"}`. The lock file itself
/// carries no content, because on Windows a held lock is mandatory and a
/// reader could not read it. The record is information for `status`; the lock
/// is the truth about whether it is held.
pub const HOLDER_FILE: &str = "mimlys.holder";

/// The runtime that holds the store's lock, as it recorded itself.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Holder {
    /// The holder's process id.
    pub pid: u32,
    /// When it took the lock (RFC 3339, UTC).
    pub since: String,
}

const MIGRATIONS: [(u32, &str); 2] = [
    (1, include_str!("migrations/001_initial.sql")),
    (2, include_str!("migrations/002_snapshots.sql")),
];

/// The SQLite store. One per data directory; holds the runtime lock for its lifetime.
pub struct Store {
    pub(crate) conn: Mutex<Connection>,
    pub(crate) data_dir: PathBuf,
    _lock: File,
    /// Transactions committed through `Store::transaction`, for tests of REQ-PERS-003.
    #[cfg(test)]
    pub(crate) transactions: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

/// Opens or creates the store under `data_dir`.
pub fn open(data_dir: &Path) -> Result<Store, PersistenceError> {
    create_private_dir(data_dir)?;
    create_private_dir(&data_dir.join("blobs"))?;
    let lock = acquire_lock(&data_dir.join("mimlys.lock"))?;
    let db_path = data_dir.join("mimlys.db");
    let conn = Connection::open(&db_path).map_err(storage("open"))?;
    restrict_file(&db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;",
    )
    .map_err(storage("pragmas"))?;
    conn.busy_timeout(Duration::from_millis(LIM_O3_LOCK_WAIT_MS))
        .map_err(storage("busy_timeout"))?;
    migrate(&conn)?;
    // Last, so that a refusal above leaves no record of a holder that is gone.
    record_holder(data_dir);
    Ok(Store {
        conn: Mutex::new(conn),
        data_dir: data_dir.to_path_buf(),
        _lock: lock,
        #[cfg(test)]
        transactions: std::sync::atomic::AtomicUsize::new(0),
    })
}

impl Drop for Store {
    /// The holder record goes while the lock is still held (the field is
    /// released after this runs), so a new holder's record is never removed.
    fn drop(&mut self) {
        let _ = fs::remove_file(self.data_dir.join(HOLDER_FILE));
    }
}

/// Records this process as the holder, atomically; failing to is not an error
/// (the record is information for readers, the lock is the guard).
fn record_holder(data_dir: &Path) {
    let holder = Holder {
        pid: std::process::id(),
        since: mimlys_core::clock::Clock::now(&mimlys_core::clock::SystemClock).to_rfc3339(),
    };
    let Ok(text) = serde_json::to_string(&holder) else {
        return;
    };
    let tmp = data_dir.join(format!("{HOLDER_FILE}.tmp"));
    let target = data_dir.join(HOLDER_FILE);
    if fs::write(&tmp, text).is_ok() && restrict_file(&tmp).is_ok() {
        let _ = fs::rename(&tmp, &target);
    }
}

impl Store {
    /// The data directory.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The journal mode and synchronous setting in force, for tests and `status`.
    pub fn pragmas(&self) -> Result<(String, i64), PersistenceError> {
        let conn = self.conn.lock().map_err(|_| poisoned())?;
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .map_err(storage("pragma"))?;
        let sync: i64 = conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .map_err(storage("pragma"))?;
        Ok((journal, sync))
    }

    /// Runs `f` with the raw connection (tests and integrity rules).
    pub(crate) fn with_conn<R>(
        &self,
        f: impl FnOnce(&mut Connection) -> R,
    ) -> Result<R, PersistenceError> {
        let mut conn = self.conn.lock().map_err(|_| poisoned())?;
        Ok(f(&mut conn))
    }

    /// The schema version stored in the database.
    pub fn schema_version(&self) -> Result<u32, PersistenceError> {
        let conn = self.conn.lock().map_err(|_| poisoned())?;
        read_version(&conn)
    }
}

pub(crate) fn migrate(conn: &Connection) -> Result<(), PersistenceError> {
    let exists: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .map_err(storage("schema_lookup"))?;
    let mut current = if exists == 0 { 0 } else { read_version(conn)? };
    if current > SCHEMA_VERSION {
        return Err(PersistenceError::NewerSchema {
            found: current,
            supported: SCHEMA_VERSION,
        });
    }
    for (version, sql) in MIGRATIONS {
        if version > current {
            let tx = conn
                .unchecked_transaction()
                .map_err(storage("migration_begin"))?;
            tx.execute_batch(sql).map_err(storage("migration"))?;
            if version > 1 {
                tx.execute("UPDATE schema_version SET version = ?1", [version])
                    .map_err(storage("migration_version"))?;
            }
            tx.commit().map_err(storage("migration_commit"))?;
            current = version;
        }
    }
    Ok(())
}

pub(crate) fn read_version(conn: &Connection) -> Result<u32, PersistenceError> {
    conn.query_row("SELECT version FROM schema_version", [], |r| {
        r.get::<_, i64>(0)
    })
    .map(|v| v as u32)
    .map_err(storage("schema_version"))
}

fn acquire_lock(path: &Path) -> Result<File, PersistenceError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(io("lock_open"))?;
    restrict_file(path)?;
    let deadline = Instant::now() + Duration::from_millis(LIM_O3_LOCK_WAIT_MS);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(PersistenceError::Locked);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(_)) => return Err(PersistenceError::Locked),
        }
    }
}

/// Creates a directory readable and writable by its owner only (REQ-WS-001).
pub(crate) fn create_private_dir(path: &Path) -> Result<(), PersistenceError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(io("create_dir"))?;
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .map_err(io("chmod_dir"))?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path).map_err(io("create_dir"))?;
    }
    Ok(())
}

/// Makes a file readable and writable by its owner only (REQ-SEC-008).
pub(crate) fn restrict_file(path: &Path) -> Result<(), PersistenceError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io("chmod_file"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub(crate) fn storage(detail: &'static str) -> impl Fn(rusqlite::Error) -> PersistenceError {
    move |_| PersistenceError::Storage {
        detail: detail.to_owned(),
    }
}

pub(crate) fn io(detail: &'static str) -> impl Fn(std::io::Error) -> PersistenceError {
    move |_| PersistenceError::Storage {
        detail: detail.to_owned(),
    }
}

pub(crate) fn poisoned() -> PersistenceError {
    PersistenceError::Storage {
        detail: "connection_poisoned".into(),
    }
}
