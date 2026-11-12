//! The read-only open of the store (REQ-PERS-013, external design §16; RFC
//! 013). A command that only reads opens the database as a WAL reader
//! (SQLite's read-only mode), without the exclusive lock on `mimlys.lock`,
//! and runs no recovery, integrity check, reaping or migration: all of those
//! write. The handle, [`ReadStore`], implements [`PersistenceRead`] and
//! nothing else, so a command that holds it cannot call a writing method:
//! the type has none, and no runtime check stands in for that.
//!
//! What a reader knows without writing:
//! - who holds the lock, if anyone ([`LockState`]): found by testing the lock
//!   with a shared try-lock that is released at once. The reader never waits
//!   for the lock and never holds it while it reads; a writer that starts at
//!   that instant retries within LIM-O3 as it does for any other holder;
//! - whether a store needs recovery: the lock is free and a task is still
//!   `running` ([`ReadStore::recovery_pending`]), the very condition the
//!   writer's recovery acts on;
//! - the schema: newer than this runtime knows is refused with the reason,
//!   older is `migration pending`, neither is migrated.

use crate::sqlite::{HOLDER_FILE, Holder, LIM_O3_LOCK_WAIT_MS, SCHEMA_VERSION, storage};
use mimlys_core::spi::{PersistenceError, PersistenceRead};
use rusqlite::{Connection, OpenFlags};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Whether another process holds the store's exclusive lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockState {
    /// Nobody holds it.
    Free,
    /// A runtime holds it; its record is given when it could be read.
    Held(Option<Holder>),
    /// The lock could not be tested (a filesystem without locks); nothing is
    /// claimed either way.
    Unknown,
}

/// A read-only handle on the store. It has no writing method: reading
/// compiles, writing does not (REQ-PERS-013).
///
/// ```
/// use mimlys_core::spi::PersistenceRead;
/// fn show(store: &mimlys_persist::ReadStore) {
///     let _ = store.list_tasks();
/// }
/// ```
///
/// ```compile_fail,E0599
/// use mimlys_core::spi::Persistence;
/// fn change(store: &mimlys_persist::ReadStore, event: &mimlys_core::event::EventRecord) {
///     let _ = store.append_event(event);
/// }
/// ```
pub struct ReadStore {
    pub(crate) conn: Mutex<Connection>,
    pub(crate) data_dir: PathBuf,
    exists: bool,
    lock: LockState,
}

impl std::fmt::Debug for ReadStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadStore")
            .field("data_dir", &self.data_dir)
            .field("exists", &self.exists)
            .finish_non_exhaustive()
    }
}

/// Opens the store under `data_dir` for reading only.
///
/// A data directory with no database yet is not an error: nothing has been
/// stored, so the handle reads an empty store (`exists()` is false) and
/// creates nothing. Nothing is created, locked, recovered, checked, reaped or
/// migrated. A schema newer than this runtime's is `NewerSchema`; an older one
/// (or a database without a schema) is `MigrationPending`.
pub fn open_read_only(data_dir: &Path) -> Result<ReadStore, PersistenceError> {
    let lock = lock_state(data_dir);
    let db_path = data_dir.join("mimlys.db");
    if !db_path.is_file() {
        let conn = Connection::open_in_memory().map_err(storage("open"))?;
        crate::sqlite::migrate(&conn)?;
        return Ok(ReadStore {
            conn: Mutex::new(conn),
            data_dir: data_dir.to_path_buf(),
            exists: false,
            lock,
        });
    }
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(storage("open"))?;
    // Waits for SQLite's own short locks (a checkpoint, a WAL recovery by the
    // writer), as long as the writer's wait for the runtime lock (LIM-O3).
    conn.busy_timeout(Duration::from_millis(LIM_O3_LOCK_WAIT_MS))
        .map_err(storage("busy_timeout"))?;
    let found = schema_version(&conn)?;
    if found > SCHEMA_VERSION {
        return Err(PersistenceError::NewerSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found < SCHEMA_VERSION {
        return Err(PersistenceError::MigrationPending {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(ReadStore {
        conn: Mutex::new(conn),
        data_dir: data_dir.to_path_buf(),
        exists: true,
        lock,
    })
}

/// The schema version in the database; 0 when it has none yet.
fn schema_version(conn: &Connection) -> Result<u32, PersistenceError> {
    let has_table: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .map_err(storage("schema_lookup"))?;
    if has_table == 0 {
        return Ok(0);
    }
    crate::sqlite::read_version(conn)
}

/// Tests the lock without keeping it: a shared try-lock on the lock file,
/// released at once. A lock file that does not exist was never taken.
fn lock_state(data_dir: &Path) -> LockState {
    let Ok(file) = File::open(data_dir.join("mimlys.lock")) else {
        return LockState::Free;
    };
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = file.unlock();
            LockState::Free
        }
        Err(std::fs::TryLockError::WouldBlock) => LockState::Held(read_holder(data_dir)),
        Err(std::fs::TryLockError::Error(_)) => LockState::Unknown,
    }
}

fn read_holder(data_dir: &Path) -> Option<Holder> {
    let text = std::fs::read_to_string(data_dir.join(HOLDER_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

impl ReadStore {
    /// The data directory.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Whether the database exists. When it does not, the handle reads an
    /// empty store and nothing was created.
    pub fn exists(&self) -> bool {
        self.exists
    }

    /// Who held the lock when the store was opened.
    pub fn lock_state(&self) -> &LockState {
        &self.lock
    }

    /// Tests the lock again now (for a command that keeps reading): the same
    /// test as at the open, and as brief.
    pub fn probe_lock(&self) -> LockState {
        lock_state(&self.data_dir)
    }

    /// Whether the store needs recovery: nobody holds the lock and a task is
    /// still `running` or a snapshot journal is pending, which the next
    /// command that writes recovers (external design §16.3; REQ-WS-012).
    /// What is committed is read as it stands; nothing is removed.
    pub fn recovery_pending(&self) -> Result<bool, PersistenceError> {
        if self.lock != LockState::Free {
            return Ok(false);
        }
        if !self.tasks_in_state("running")?.is_empty() {
            return Ok(true);
        }
        Ok(
            !crate::snapshots::SnapshotFiles::open_for_reading(&self.data_dir)
                .pending()?
                .is_empty(),
        )
    }

    /// The schema version in the database.
    pub fn schema_version(&self) -> Result<u32, PersistenceError> {
        let conn = self.conn.lock().map_err(|_| crate::sqlite::poisoned())?;
        crate::sqlite::read_version(&conn)
    }
}

#[cfg(test)]
mod tests;
