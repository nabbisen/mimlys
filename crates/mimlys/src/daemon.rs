//! The daemon (RFC 014 with Amendment 1; external design §19.5; REQ-ARCH-009, REQ-PERS-014,
//! REQ-SEC-016, REQ-SCHED-008, REQ-CLI-008).
//!
//! One daemon per operating-system user and data directory serves the control API to that user's
//! clients: over a Unix socket on Linux and macOS, over a named pipe on Windows. A client that
//! finds none runs embedded, as before.
//!
//! - [`wire`]: what the two ends say to each other: the hellos and `stop`, whose shapes never
//!   change, and the requests after them. The same on every platform.
//! - [`socket`] (Unix) and [`pipe`] (Windows): the transport, who may reach it, and the daemon's
//!   own tree. Each is also [`plat`], the one name the rest of the daemon uses.

pub mod command;
pub mod connect;
pub mod log;
pub mod server;
pub mod transport;
pub mod wire;

/// The name of a pipe from a user's SID and a data directory (pure; every platform tests it).
pub mod pipe_name;

#[cfg(unix)]
pub mod socket;
#[cfg(unix)]
pub use socket as plat;

#[cfg(windows)]
pub mod marks;
#[cfg(windows)]
pub mod pipe;
#[cfg(windows)]
pub use pipe as plat;
