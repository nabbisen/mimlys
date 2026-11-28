//! The daemon's entry in the registry (RFC 014, Amendment 1, item 4; external design §19.5;
//! REQ-SCHED-008): a daemon is recorded, so that `status` shows it among the machine's runtimes.
//!
//! **A new kind of file, not a new kind of entry.** An entry's text is `<state>-<kind>-<id>.entry`
//! and an older release refuses any text it cannot read and **removes** it (it reaps what it
//! cannot use, and says so in its log). A daemon's record that an older release removed would
//! disappear from the machine view while the daemon runs, and an older `status` would call it
//! unusable. So the daemon's record is `daemon-<id>.daemon`, with its lock file
//! `daemon-<id>.lock` (the same holder protocol: the file is locked for as long as the daemon
//! lives): an older release lists no `.daemon` file, and treats the lock file as one whose
//! holder is alive and leaves it (a lock file with no text, held, is not removed). A newer
//! release lists both.

use super::*;

/// The version of a daemon's record.
pub const DAEMON_ENTRY_VERSION: u32 = 1;

/// A daemon, as its record states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonEntry {
    /// [`DAEMON_ENTRY_VERSION`].
    pub v: u32,
    /// The daemon's process.
    pub pid: u32,
    /// The data directory it serves, as shown.
    pub data_dir: String,
    /// Its socket.
    pub socket: String,
    /// Its runtime's version.
    pub version: String,
    /// Since when it runs.
    pub since: Instant,
}

const SUFFIX: &str = ".daemon";

fn read_daemon(dir: &Path, stem: &str) -> Result<DaemonEntry, Reason> {
    let path = dir.join(format!("{stem}{SUFFIX}"));
    let meta = std::fs::symlink_metadata(&path).map_err(|e| io_reason("reading", &path, &e))?;
    is_own_file(&meta)?;
    if meta.len() > MAX_ENTRY_BYTES {
        return Err(format!("it is larger than {MAX_ENTRY_BYTES} bytes"));
    }
    let mut file = open_no_follow(&path, OpenOptions::new().read(true))
        .map_err(|e| io_reason("opening", &path, &e))?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_reason("reading", &path, &e))?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err(format!("it is larger than {MAX_ENTRY_BYTES} bytes"));
    }
    let entry: DaemonEntry =
        serde_json::from_slice(&bytes).map_err(|_| "its text cannot be parsed".to_owned())?;
    if entry.v != DAEMON_ENTRY_VERSION {
        return Err(format!(
            "its version {} is not {DAEMON_ENTRY_VERSION}",
            entry.v
        ));
    }
    Ok(entry)
}

/// The daemons whose holder is alive; with `reap`, the records whose holder is gone, or that
/// cannot be used, are removed.
fn scan_daemons(dir: &Path, reap: bool) -> Result<Vec<DaemonEntry>, Reason> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut live = Vec::new();
    for name in file_names(dir)? {
        let Some(stem) = name.strip_suffix(SUFFIX) else {
            continue;
        };
        if !stem.starts_with("daemon-") {
            continue;
        }
        let seen = read_daemon(dir, stem)
            .and_then(|entry| holder_alive(dir, stem).map(|gone| (entry, gone)));
        match seen {
            Ok((entry, None)) => live.push(entry),
            Ok((_, Some(_))) | Err(_) => {
                if reap {
                    let _ = std::fs::remove_file(dir.join(&name));
                    if lock_may_go(dir, stem) {
                        let _ = std::fs::remove_file(dir.join(format!("{stem}.lock")));
                    }
                }
            }
        }
    }
    live.sort_by_key(|d| d.pid);
    Ok(live)
}

impl Registry {
    /// Records a daemon: its lock file created and locked, its text written in full and moved
    /// into place. The record stays for as long as the handle lives. The records of daemons that
    /// are gone are removed first.
    pub fn register_daemon(self: &Arc<Self>, entry: DaemonEntry) -> Result<Handle, Reason> {
        let this = Arc::clone(self);
        bounded(self.bounds, move |_| {
            if !this.prepared.load(std::sync::atomic::Ordering::SeqCst) {
                Self::prepare(&this.dir)?;
                this.prepared
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            scan_daemons(&this.dir, true)?;
            let id = uuid::Uuid::now_v7().simple().to_string();
            let stem = format!("daemon-{id}");
            let lock_path = this.dir.join(format!("{stem}.lock"));
            let lock = create_new_no_follow(&lock_path)
                .map_err(|e| io_reason("creating", &lock_path, &e))?;
            let handle = Handle {
                dir: this.dir.clone(),
                stem: stem.clone(),
                lock: Some(lock),
                text: SUFFIX,
            };
            if handle.lock.as_ref().expect("just made").try_lock().is_err() {
                return Err(format!("locking {} failed", lock_path.display()));
            }
            let text = serde_json::to_vec(&entry)
                .map_err(|_| "a daemon's record cannot be written".to_owned())?;
            let tmp = this.dir.join(format!("{stem}{SUFFIX}.tmp"));
            let mut file =
                create_new_no_follow(&tmp).map_err(|e| io_reason("creating", &tmp, &e))?;
            std::io::Write::write_all(&mut file, &text)
                .map_err(|e| io_reason("writing", &tmp, &e))?;
            drop(file);
            let path = this.dir.join(format!("{stem}{SUFFIX}"));
            std::fs::rename(&tmp, &path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                io_reason("moving", &path, &e)
            })?;
            Ok(handle)
        })
    }

    /// The daemons that run, read and changed by no one.
    pub fn daemons(&self) -> Result<Vec<DaemonEntry>, Reason> {
        let dir = self.dir.clone();
        bounded(self.bounds, move |_| scan_daemons(&dir, false))
    }

    /// The daemon that serves the data directory `data_dir` (as shown), when one runs.
    pub fn daemon_of(&self, data_dir: &str) -> Result<Option<DaemonEntry>, Reason> {
        Ok(self.daemons()?.into_iter().find(|d| d.data_dir == data_dir))
    }
}
