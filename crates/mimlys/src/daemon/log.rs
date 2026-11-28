//! The daemon's log (RFC 014, Amendment 1, item 10; external design §19.5; LIM-O20, LIM-O21): a
//! file under the data directory, `logs/daemon.log`, bounded in size and in the number of files:
//! past LIM-O20 it is renamed `daemon.log.1`, the older copies move up, and the copy past
//! LIM-O21 is dropped. It holds the lines of the runtime's log (redacted as it is on standard
//! error) and the connections, which are not events.

use mimlys_core::limits::{LIM_O20_DAEMON_LOG_BYTES, LIM_O21_DAEMON_LOG_COPIES};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The log's directory under the data directory.
pub const LOG_DIR: &str = "logs";
/// The log's file name.
pub const LOG_FILE: &str = "daemon.log";

/// The log file of a data directory.
pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG_DIR).join(LOG_FILE)
}

struct State {
    file: File,
    size: u64,
}

/// A log file that rotates by size.
pub struct RotatingLog {
    path: PathBuf,
    max_bytes: u64,
    copies: usize,
    state: Mutex<State>,
}

fn open_append(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

impl RotatingLog {
    /// Opens (and makes, mode 0700 for the directory and 0600 for the file) the log at `path`
    /// with the bounds of LIM-O20 and LIM-O21.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Self::open_bounded(path, LIM_O20_DAEMON_LOG_BYTES, LIM_O21_DAEMON_LOG_COPIES)
    }

    /// As [`RotatingLog::open`], with other bounds (the tests').
    pub fn open_bounded(path: &Path, max_bytes: u64, copies: usize) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            mimlys_exec::confine::create_private_dir(dir)?;
        }
        let file = open_append(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path: path.to_owned(),
            max_bytes,
            copies,
            state: Mutex::new(State { file, size }),
        })
    }

    fn copy_path(&self, n: usize) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    /// Writes `bytes`, rotating first when they would take the file past its bound. A line is
    /// never split between two files.
    pub fn write_line(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.size > 0 && state.size + bytes.len() as u64 > self.max_bytes {
            // The oldest copy goes, the others move up, and the file becomes the first copy.
            let _ = std::fs::remove_file(self.copy_path(self.copies));
            for n in (1..self.copies).rev() {
                let _ = std::fs::rename(self.copy_path(n), self.copy_path(n + 1));
            }
            if self.copies == 0 {
                let _ = std::fs::remove_file(&self.path);
            } else {
                std::fs::rename(&self.path, self.copy_path(1))?;
            }
            state.file = open_append(&self.path)?;
            state.size = 0;
        }
        state.file.write_all(bytes)?;
        state.size += bytes.len() as u64;
        Ok(())
    }
}

/// The last `lines` lines of the log at `path`, from at most its last mebibyte; none when there
/// is no log yet.
pub fn tail(path: &Path, lines: usize) -> Vec<String> {
    const WINDOW: u64 = 1024 * 1024;
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let start = len.saturating_sub(WINDOW);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.take(WINDOW).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut all: Vec<&str> = text.lines().collect();
    if start > 0 && !all.is_empty() {
        // The first line of a window that began in the middle of one is not whole.
        all.remove(0);
    }
    let from = all.len().saturating_sub(lines);
    all[from..].iter().map(|l| (*l).to_owned()).collect()
}

#[cfg(test)]
mod tests;
