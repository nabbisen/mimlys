//! The registry of the user's runtimes (RFC 013 decisions 5, 7 and 8; external design §19.3;
//! REQ-SCHED-006, REQ-SCHED-007, REQ-OBS-009; THR-37, THR-38).
//!
//! One directory per operating-system user, under the user's runtime directory and outside every
//! confinement's write set, holds one **entry** for each execution slot taken, each engine gate
//! held and each runtime that waits for either. An entry is two files:
//!
//! - `<state>-<kind>-<id>.lock`: empty, created and **locked** by its holder, and kept locked for
//!   as long as the holder lives. The operating system drops the lock when the holder ends, however
//!   it ends, so a lock that is free is a holder that is gone: that entry is stale.
//! - `<state>-<kind>-<id>.entry`: the entry's text (a small JSON object, [`Entry`]), written in
//!   full before it appears, and never changed. It is a separate file because on Windows a locked
//!   file cannot be read by another process.
//!
//! `registry.lock` is the mutex of the registry's writers: an operation that counts and then adds
//! (a take) or removes (a reap) holds it, so two runtimes cannot both see a free slot. Readers
//! (`status`, `task show`) take nothing and change nothing.
//!
//! Every operation is bounded by LIM-O11 ([`Bounds::op`]), including the I/O of the filesystem,
//! which runs on a thread of its own and is given up at the bound; a take that completes after it
//! was given up drops what it made. The registry never follows a link, refuses a file that is not
//! the user's own regular file, and refuses (reaps, and says so) an entry it cannot parse.

mod admission;
mod daemon;
#[cfg(test)]
mod tests;

pub use admission::{Wait, gate, gate_in, slots, slots_in};
pub use daemon::{DAEMON_ENTRY_VERSION, DaemonEntry};

use mimlys_core::clock::{Clock, Instant};
use mimlys_core::limits::LIM_O11_REGISTRY_MS;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The version of an entry's text.
pub const ENTRY_VERSION: u32 = 1;
/// The most an entry may hold, in bytes; a larger file is refused.
const MAX_ENTRY_BYTES: u64 = 4096;
/// The most files the registry reads in one scan; a directory with more is not usable.
const MAX_FILES: usize = 4096;
/// The environment variable that names the registry's directory instead of the platform's. It is
/// a seam for tests, read only in builds with `debug_assertions` on (like the `MIMLYS_PROBE_*`
/// knobs) and never in a release: a user who set it for one runtime and not for another would
/// split the registry in two without seeing it (013d-4).
pub const DIR_ENV: &str = "MIMLYS_RUN_DIR";

/// Whether the registry's directory is there, as a metadata look finds it ([`Registry::check`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// The directory exists and nothing in its metadata keeps this user from using it.
    Present,
    /// No runtime of the user has taken a slot yet: the first take makes the directory.
    Absent,
}

/// What an entry is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An execution slot (LIM-X5).
    Slot,
    /// An engine gate (LIM-O12).
    Gate,
}

impl Kind {
    /// The word in a file name and in the entry's text.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Slot => "slot",
            Kind::Gate => "gate",
        }
    }
}

/// Whether the runtime holds what the entry names or waits for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The runtime holds a slot or a gate; it counts against the limit.
    Held,
    /// The runtime waits for one; it counts against nothing.
    Waiting,
}

impl State {
    /// The word in a file name and in the entry's text.
    pub fn name(self) -> &'static str {
        match self {
            State::Held => "held",
            State::Waiting => "waiting",
        }
    }
}

/// One entry of the registry, as its file states it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// [`ENTRY_VERSION`].
    pub v: u32,
    /// What it is for.
    pub kind: Kind,
    /// Held, or waited for.
    pub state: State,
    /// The runtime's process.
    pub pid: u32,
    /// The runtime's data directory.
    pub data_dir: String,
    /// The task whose execution or request it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Since when it is held or waited for.
    pub since: Instant,
    /// The limit in force for the runtime that wrote it: `admission_slots` or `max_in_flight`.
    pub limit: u32,
    /// A gate: the key of its endpoint (what two runtimes of one user share).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A gate: the endpoint as shown (no credentials, no path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// What the registry is asked for: a slot, or the gate of one endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Class {
    /// Slot or gate.
    pub kind: Kind,
    /// A gate: the key of its endpoint.
    pub key: Option<String>,
    /// A gate: the endpoint as shown.
    pub url: Option<String>,
    /// How many may be held at once.
    pub limit: usize,
}

impl Class {
    /// Whether `entry` is of this class.
    fn matches(&self, entry: &Entry) -> bool {
        entry.kind == self.kind && entry.key == self.key
    }
}

/// The endpoint a gate is for: scheme, host and port, no credentials, no path, no query, in
/// lower case. Two URLs of one engine give the same one.
pub fn endpoint_of(url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let port = parsed.port_or_known_default()?;
    let shown = format!("{}://{}:{}", parsed.scheme(), host, port);
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(shown.as_bytes());
    let key: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    Some((key, shown))
}

/// What an operation says when the writers' mutex stays held for the whole of its wait (013f).
///
/// It says what the code knows: `registry.lock` is locked by some open file description. That is
/// another operation of this runtime (its own `take` and `reap` run on threads of their own), an
/// operation of another runtime, or, for the few milliseconds of a start, a child process that
/// inherited the descriptor and has not executed yet (an operating system keeps a lock until
/// every copy of the descriptor is closed). The code cannot tell these apart, so it names none.
pub const LOCK_HELD: &str =
    "the registry's lock is held by another operation (of this runtime or another)";

/// The share of an operation's bound that the wait for the writers' mutex may take by default.
/// What is left is the margin in which a mutex that stays held is told as that, before the
/// operation is given up as one that did not answer.
const LOCK_SHARE: f32 = 0.75;

/// The bounds of the registry's operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    /// One operation: take, return, reap, list (LIM-O11).
    pub op: Duration,
    /// How long an operation that writes waits for the writers' mutex, measured from the start
    /// of the operation: less than `op`. The difference is the margin in which a held mutex is
    /// told as [`LOCK_HELD`]; only a filesystem that does not answer within `op` is told as a
    /// registry that did not answer.
    pub lock: Duration,
    /// How often a wait looks again.
    pub poll: Duration,
}

impl Bounds {
    /// The bounds of an operation of at most `op`: it waits for the mutex for three quarters of
    /// that, and a wait looks again every `poll`.
    pub fn new(op: Duration, poll: Duration) -> Self {
        Self {
            op,
            lock: op.mul_f32(LOCK_SHARE),
            poll,
        }
    }
}

impl Default for Bounds {
    fn default() -> Self {
        Self::new(
            Duration::from_millis(LIM_O11_REGISTRY_MS),
            Duration::from_millis(200),
        )
    }
}

/// Who asks: the runtime that writes entries.
#[derive(Clone, Debug)]
pub struct Owner {
    /// The process.
    pub pid: u32,
    /// The data directory, as shown.
    pub data_dir: String,
}

/// What [`DIR_ENV`] names, when this build reads it: only with `debug_assertions` on.
fn seam_dir() -> Option<std::ffi::OsString> {
    seam_of(std::env::var_os(DIR_ENV))
}

/// What the seam's value counts for: itself (if not empty) in a debug build, nothing in a release.
fn seam_of(value: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    if cfg!(debug_assertions) {
        value.filter(|d| !d.is_empty())
    } else {
        None
    }
}

/// The directory of the registry on this machine: [`DIR_ENV`] when it is set (a debug build
/// only, [`seam_dir`]), else the user's runtime directory (`$XDG_RUNTIME_DIR/mimlys`, else
/// `$XDG_DATA_HOME/mimlys/run`; `%LOCALAPPDATA%\mimlys\run`;
/// `~/Library/Application Support/mimlys/run`).
pub fn default_dir() -> Result<PathBuf, String> {
    if let Some(dir) = seam_dir() {
        let dir = PathBuf::from(dir);
        return if dir.is_absolute() {
            Ok(dir)
        } else {
            Err(format!("{DIR_ENV} is not an absolute path"))
        };
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        && runtime.is_absolute()
        && runtime.is_dir()
    {
        return Ok(runtime.join("mimlys"));
    }
    let dirs = directories::BaseDirs::new()
        .ok_or_else(|| "no home directory is known for this user".to_owned())?;
    #[cfg(windows)]
    let base = dirs.data_local_dir().to_path_buf();
    #[cfg(not(windows))]
    let base = dirs.data_dir().to_path_buf();
    Ok(base.join("mimlys").join("run"))
}

/// The registry: a directory, the bounds of its operations and the clock that stamps its entries.
pub struct Registry {
    dir: PathBuf,
    bounds: Bounds,
    clock: Arc<dyn Clock>,
    /// The directory was made private in this process (once is enough).
    prepared: std::sync::atomic::AtomicBool,
}

/// An entry held by this process: its file stays locked until the handle is dropped, which
/// removes it (the lock first, so that it reads as stale at once if the removal fails).
#[derive(Debug)]
pub struct Handle {
    dir: PathBuf,
    stem: String,
    lock: Option<File>,
    /// The extension of the text beside the lock file: `.entry`, or `.daemon` for a daemon's.
    text: &'static str,
}

impl Handle {
    /// The entry's file stem.
    pub fn stem(&self) -> &str {
        &self.stem
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // Closing the file releases the lock: from here the entry is stale, whatever happens.
        drop(self.lock.take());
        let _ = std::fs::remove_file(self.dir.join(format!("{}{}", self.stem, self.text)));
        let _ = std::fs::remove_file(self.dir.join(format!("{}.lock", self.stem)));
    }
}

/// An entry found by a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The file stem.
    pub stem: String,
    /// What it says.
    pub entry: Entry,
}

/// An entry that was removed (or, for a reader, would be) and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reaped {
    /// The file stem.
    pub stem: String,
    /// What it said, when it could be read.
    pub entry: Option<Entry>,
    /// Why: `its holder is gone`, or what is wrong with the file.
    pub reason: String,
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    /// Entries whose holder is alive.
    pub live: Vec<Seen>,
    /// Entries whose holder is gone, or that cannot be used: reaped by a writer's scan.
    pub reaped: Vec<Reaped>,
}

/// The result of a take.
pub enum Take {
    /// A slot or a gate is held now.
    Granted {
        /// The held entry.
        handle: Handle,
        /// Held entries of the class now, this one included.
        taken: u32,
    },
    /// All of them are held.
    Busy {
        /// Held entries of the class now.
        taken: u32,
        /// Their processes.
        holders: Vec<u32>,
        /// This runtime's waiting entry (kept for the next look).
        waiter: Handle,
    },
}

type Reason = String;

fn io_reason(what: &str, path: &Path, error: &std::io::Error) -> Reason {
    format!("{what} {} failed ({})", path.display(), error.kind())
}

/// Opens `path` without following a link at its last component.
fn open_no_follow(path: &Path, options: &mut OpenOptions) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        options.open(path)
    }
    #[cfg(not(unix))]
    {
        // A link (a symbolic link or a junction) at the last component is refused before the
        // open; what the filesystem does between the two is the same user's own doing.
        if let Ok(meta) = std::fs::symlink_metadata(path)
            && is_link(&meta)
        {
            return Err(std::io::Error::other("a link"));
        }
        options.open(path)
    }
}

#[cfg(not(unix))]
fn is_link(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    meta.file_type().is_symlink() || meta.file_attributes() & REPARSE_POINT != 0
}

fn create_new_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    open_no_follow(path, &mut options)
}

/// Whether `meta` is a regular file of this user (a link, a directory or another user's file is
/// not an entry).
fn is_own_file(meta: &std::fs::Metadata) -> Result<(), Reason> {
    if !meta.file_type().is_file() {
        return Err("it is not a regular file".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err("it belongs to another user".to_owned());
        }
    }
    Ok(())
}

/// Reads an entry's file: a regular file of this user within the bound, whose text parses and
/// names what its file name says.
fn read_entry(dir: &Path, stem: &str) -> Result<Entry, Reason> {
    let path = dir.join(format!("{stem}.entry"));
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
    let entry: Entry =
        serde_json::from_slice(&bytes).map_err(|_| "its text cannot be parsed".to_owned())?;
    if entry.v != ENTRY_VERSION {
        return Err(format!("its version {} is not {ENTRY_VERSION}", entry.v));
    }
    let mut parts = stem.splitn(3, '-');
    let (state, kind) = (parts.next(), parts.next());
    if state != Some(entry.state.name()) || kind != Some(entry.kind.name()) {
        return Err("its name and its text disagree".to_owned());
    }
    if entry.kind == Kind::Gate && entry.key.is_none() {
        return Err("a gate without an endpoint".to_owned());
    }
    Ok(entry)
}

/// Whether the holder of the entry named `stem` is alive: its lock file is locked.
fn holder_alive(dir: &Path, stem: &str) -> Result<Option<Reason>, Reason> {
    let path = dir.join(format!("{stem}.lock"));
    let file = match open_no_follow(&path, OpenOptions::new().read(true)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some("its lock file is gone".to_owned()));
        }
        Err(e) => return Err(io_reason("opening", &path, &e)),
    };
    match file.try_lock_shared() {
        // Nobody holds it: the holder is gone. The probe's own lock goes at once.
        Ok(()) => {
            let _ = file.unlock();
            Ok(Some("its holder is gone".to_owned()))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(io_reason("locking", &path, &e)),
    }
}

/// Whether a lock file whose entry is gone or unusable may be removed: its holder is gone, or it
/// is not a regular file (a link or a directory put there is removed as what it is, not
/// followed). A lock file that is held stays until its holder ends.
fn lock_may_go(dir: &Path, stem: &str) -> bool {
    match holder_alive(dir, stem) {
        Ok(gone) => gone.is_some(),
        Err(_) => std::fs::symlink_metadata(dir.join(format!("{stem}.lock")))
            .map(|meta| !meta.file_type().is_file())
            .unwrap_or(false),
    }
}

fn file_names(dir: &Path) -> Result<Vec<String>, Reason> {
    let mut names = Vec::new();
    for item in std::fs::read_dir(dir).map_err(|e| io_reason("listing", dir, &e))? {
        let item = item.map_err(|e| io_reason("listing", dir, &e))?;
        if names.len() >= MAX_FILES {
            return Err(format!("it holds more than {MAX_FILES} files"));
        }
        if let Some(name) = item.file_name().to_str() {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Looks at every entry. With `reap` (the caller holds the mutex) the stale and the unusable ones
/// are removed; without it nothing is changed, and they are listed all the same.
fn scan(dir: &Path, reap: bool) -> Result<Scan, Reason> {
    let mut result = Scan::default();
    let names = file_names(dir)?;
    let remove = |stem: &str| {
        let _ = std::fs::remove_file(dir.join(format!("{stem}.entry")));
        let _ = std::fs::remove_file(dir.join(format!("{stem}.lock")));
    };
    for name in &names {
        if let Some(stem) = name.strip_suffix(".entry") {
            // Its text, then its holder: a text that cannot be used, and a lock file that
            // cannot be probed (a link, say), are an entry that cannot be used.
            let seen = read_entry(dir, stem)
                .and_then(|entry| holder_alive(dir, stem).map(|gone| (entry, gone)));
            match seen {
                Ok((entry, None)) => result.live.push(Seen {
                    stem: stem.to_owned(),
                    entry,
                }),
                Ok((entry, Some(reason))) => {
                    if reap {
                        remove(stem);
                    }
                    result.reaped.push(Reaped {
                        stem: stem.to_owned(),
                        entry: Some(entry),
                        reason,
                    });
                }
                Err(why) => {
                    if reap {
                        // The text goes; a lock file still held goes when its holder does.
                        let _ = std::fs::remove_file(dir.join(name));
                        if lock_may_go(dir, stem) {
                            let _ = std::fs::remove_file(dir.join(format!("{stem}.lock")));
                        }
                    }
                    result.reaped.push(Reaped {
                        stem: stem.to_owned(),
                        entry: None,
                        reason: format!("it cannot be used: {why}"),
                    });
                }
            }
        } else if let Some(stem) = name.strip_suffix(".lock") {
            // A lock file with no text is what a take that ended in the middle left.
            if name != "registry.lock"
                && !names.iter().any(|n| n == &format!("{stem}.entry"))
                && reap
                && lock_may_go(dir, stem)
            {
                let _ = std::fs::remove_file(dir.join(name));
            }
        } else if name.ends_with(".entry.tmp") && reap {
            let _ = std::fs::remove_file(dir.join(name));
        }
    }
    Ok(result)
}

/// Takes the writers' mutex, waiting at most until `deadline`.
fn lock_mutex(dir: &Path, deadline: std::time::Instant) -> Result<File, Reason> {
    let path = dir.join("registry.lock");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = open_no_follow(&path, &mut options).map_err(|e| io_reason("opening", &path, &e))?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(LOCK_HELD.to_owned());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(io_reason("locking", &path, &e)),
        }
    }
}

/// Makes an entry: its lock file created and locked, its text written in full and then moved into
/// place, so that no reader sees a part of it. The caller holds the mutex.
fn create(dir: &Path, entry: &Entry) -> Result<Handle, Reason> {
    let id = uuid::Uuid::now_v7().simple().to_string();
    let stem = format!("{}-{}-{id}", entry.state.name(), entry.kind.name());
    let lock_path = dir.join(format!("{stem}.lock"));
    let lock =
        create_new_no_follow(&lock_path).map_err(|e| io_reason("creating", &lock_path, &e))?;
    let handle = Handle {
        dir: dir.to_owned(),
        stem: stem.clone(),
        lock: Some(lock),
        text: ".entry",
    };
    match handle.lock.as_ref().expect("just made").try_lock() {
        Ok(()) => {}
        Err(_) => return Err(format!("locking {} failed", lock_path.display())),
    }
    let text = serde_json::to_vec(entry).map_err(|_| "an entry cannot be written".to_owned())?;
    let tmp = dir.join(format!("{stem}.entry.tmp"));
    let mut file = create_new_no_follow(&tmp).map_err(|e| io_reason("creating", &tmp, &e))?;
    std::io::Write::write_all(&mut file, &text).map_err(|e| io_reason("writing", &tmp, &e))?;
    drop(file);
    let path = dir.join(format!("{stem}.entry"));
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_reason("moving", &path, &e)
    })?;
    Ok(handle)
}

/// Runs `work` on a thread of its own and gives it up at the operation's bound: a filesystem
/// that does not answer cannot hold the caller (LIM-O11). What the work returns after it was
/// given up is dropped on its thread, which releases anything it made.
///
/// The work is given the instant at which its wait for the writers' mutex ends. It is measured
/// here, from the start of the operation and before the thread starts, never inside the thread:
/// a thread that starts late (a loaded machine) then has less time to wait and never more than
/// the operation has left, so that a mutex that is held is reported as held, and "did not
/// answer" is left to a filesystem that did not.
fn bounded<T: Send + 'static>(
    bounds: Bounds,
    work: impl FnOnce(std::time::Instant) -> Result<T, Reason> + Send + 'static,
) -> Result<T, Reason> {
    bounded_from(std::time::Instant::now(), bounds, work)
}

/// [`bounded`] for an operation that began at `start`.
fn bounded_from<T: Send + 'static>(
    start: std::time::Instant,
    bounds: Bounds,
    work: impl FnOnce(std::time::Instant) -> Result<T, Reason> + Send + 'static,
) -> Result<T, Reason> {
    let lock_deadline = start + bounds.lock.min(bounds.op);
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("registry".into())
        .spawn(move || {
            let _ = sender.send(work(lock_deadline));
        })
        .map_err(|_| "a thread for the registry cannot be started".to_owned())?;
    match receiver.recv_timeout(bounds.op.saturating_sub(start.elapsed())) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "the registry did not answer within {} ms",
            bounds.op.as_millis()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("the registry's thread ended without an answer".to_owned())
        }
    }
}

impl Registry {
    /// A registry in `dir`. Nothing is read or made until it is used.
    pub fn new(dir: PathBuf, bounds: Bounds, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            dir,
            bounds,
            clock,
            prepared: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The bounds.
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Makes the directory the user's own (once per process): not a link, owner-only.
    fn prepare(dir: &Path) -> Result<(), Reason> {
        if let Ok(meta) = std::fs::symlink_metadata(dir) {
            #[cfg(unix)]
            let link = meta.file_type().is_symlink();
            #[cfg(not(unix))]
            let link = is_link(&meta);
            if link || !meta.is_dir() {
                return Err(format!("{} is not a directory of its own", dir.display()));
            }
        }
        mimlys_exec::confine::create_private_dir(dir).map_err(|e| {
            format!(
                "the directory {} cannot be made private ({})",
                dir.display(),
                e.kind()
            )
        })
    }

    /// Takes a slot or a gate of `class`, or finds all of them held. `waiter` is this runtime's
    /// waiting entry from the last look, if any; it is dropped when the take is granted.
    pub async fn take(
        self: &Arc<Self>,
        class: &Class,
        task_id: Option<String>,
        owner: &Owner,
        waiter: Option<Handle>,
    ) -> Result<Take, Reason> {
        let this = Arc::clone(self);
        let (class, owner) = (class.clone(), owner.clone());
        let bounds = self.bounds;
        tokio::task::spawn_blocking(move || {
            bounded(bounds, move |deadline| {
                this.take_now(&class, task_id, &owner, waiter, deadline)
            })
        })
        .await
        .map_err(|_| "the registry's thread ended without an answer".to_owned())?
    }

    fn take_now(
        &self,
        class: &Class,
        task_id: Option<String>,
        owner: &Owner,
        waiter: Option<Handle>,
        deadline: std::time::Instant,
    ) -> Result<Take, Reason> {
        if !self.prepared.load(std::sync::atomic::Ordering::SeqCst) {
            Self::prepare(&self.dir)?;
            self.prepared
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let _mutex = lock_mutex(&self.dir, deadline)?;
        let scanned = scan(&self.dir, true)?;
        for reaped in &scanned.reaped {
            tracing::info!(
                target: "mimlys",
                entry = %reaped.stem,
                reason = %reaped.reason,
                "a stale entry of the registry was removed"
            );
        }
        let holders: Vec<&Seen> = scanned
            .live
            .iter()
            .filter(|s| class.matches(&s.entry) && s.entry.state == State::Held)
            .collect();
        let taken = holders.len() as u32;
        let entry = |state: State| Entry {
            v: ENTRY_VERSION,
            kind: class.kind,
            state,
            pid: owner.pid,
            data_dir: owner.data_dir.clone(),
            task_id: task_id.clone(),
            since: self.clock.now(),
            limit: class.limit as u32,
            key: class.key.clone(),
            url: class.url.clone(),
        };
        if holders.len() < class.limit {
            drop(waiter);
            let handle = create(&self.dir, &entry(State::Held))?;
            return Ok(Take::Granted {
                handle,
                taken: taken + 1,
            });
        }
        let mut pids: Vec<u32> = holders.iter().map(|s| s.entry.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        let waiter = match waiter {
            Some(waiter) => waiter,
            None => create(&self.dir, &entry(State::Waiting))?,
        };
        Ok(Take::Busy {
            taken,
            holders: pids,
            waiter,
        })
    }

    /// Every live entry, read and changed by no one: what `status` and `task show` show. A
    /// directory that is not there yet holds nothing. Entries whose holder is gone are listed
    /// as `reaped`, though a reader removes none.
    pub fn list(&self) -> Result<Scan, Reason> {
        let dir = self.dir.clone();
        bounded(self.bounds, move |_| {
            if !dir.exists() {
                return Ok(Scan::default());
            }
            scan(&dir, false)
        })
    }

    /// Removes the stale and the unusable entries and lists what it removed (`mimlys runtime
    /// reap`; also what every take does first).
    pub fn reap(&self) -> Result<Vec<Reaped>, Reason> {
        let dir = self.dir.clone();
        bounded(self.bounds, move |deadline| {
            if !dir.exists() {
                return Ok(Vec::new());
            }
            let _mutex = lock_mutex(&dir, deadline)?;
            Ok(scan(&dir, true)?.reaped)
        })
    }

    /// Whether a slot could be taken here, judged from the directory's metadata alone: it
    /// writes nothing, makes nothing and removes nothing (013d-4; a read command only reads).
    /// `status` asks, so that a registry that cannot be used is shown before a task meets it.
    /// The first take still decides for real: it makes the directory private, which a metadata
    /// look cannot promise (a read-only filesystem, say, is found only by writing).
    pub fn check(&self) -> Result<Presence, Reason> {
        let dir = self.dir.clone();
        bounded(self.bounds, move |_| Self::look(&dir))
    }

    fn look(dir: &Path) -> Result<Presence, Reason> {
        let meta = match std::fs::symlink_metadata(dir) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Presence::Absent),
            Err(e) => return Err(io_reason("looking at", dir, &e)),
        };
        #[cfg(unix)]
        let link = meta.file_type().is_symlink();
        #[cfg(not(unix))]
        let link = is_link(&meta);
        if link || !meta.is_dir() {
            return Err(format!("{} is not a directory of its own", dir.display()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // Another user's directory cannot be made private by this one.
            if meta.uid() != rustix::process::geteuid().as_raw() {
                return Err(format!("{} belongs to another user", dir.display()));
            }
        }
        Ok(Presence::Present)
    }
}
