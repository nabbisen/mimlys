//! The daemon on Linux and macOS (RFC 014 with Amendment 1; external design §19.5; REQ-ARCH-009,
//! REQ-PERS-014, REQ-SEC-016, REQ-SCHED-008, REQ-CLI-008).
//!
//! One daemon per operating-system user and data directory serves the control API to that user's
//! clients over a Unix socket. A client that finds none runs embedded, as before.
//!
//! - [`wire`]: what the two ends say to each other: the hellos and `stop`, whose shapes never
//!   change, and the requests after them.
//! - [`socket`] (Unix): the socket, its directory, the peer's credentials, the daemon's own tree.

pub mod wire;

#[cfg(unix)]
pub mod command;
#[cfg(unix)]
pub mod connect;
#[cfg(unix)]
pub mod log;
#[cfg(unix)]
pub mod server;

#[cfg(unix)]
pub mod socket;
