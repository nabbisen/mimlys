//! A pseudo-terminal for a test of what a command writes to a terminal (Unix): the terminal side
//! for a child's standard output, and a thread that collects what the child writes to it. The
//! terminal turns every line feed into a carriage return and a line feed (as one does), which
//! the tests that read it account for.

use std::io::Read;
use std::sync::{Arc, Mutex};

/// A pseudo-terminal.
pub struct Terminal {
    /// The terminal side, which a child writes to.
    pub slave: std::fs::File,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Terminal {
    /// Opens one.
    pub fn open() -> Self {
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("a pty master");
        grantpt(&master).expect("grantpt");
        unlockpt(&master).expect("unlockpt");
        let name = ptsname(&master, Vec::new()).expect("the pty's name");
        // The terminal side is opened first, so that nothing written to it is lost.
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name.to_str().expect("a path"))
            .expect("the pty's terminal side");
        let output = Arc::new(Mutex::new(Vec::new()));
        let collected = Arc::clone(&output);
        let mut master = std::fs::File::from(master);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // Ends when the terminal side is closed (an error on some systems).
            while let Ok(n) = master.read(&mut buf) {
                if n == 0 {
                    break;
                }
                collected.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Self { slave, output }
    }

    /// What the child has written so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }
}
