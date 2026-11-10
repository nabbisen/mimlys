//! The mimlys command line and runtime host (RFC 010; external design §19).
//!
//! The binary embeds the runtime: one process per command, the data
//! directory locked while it runs, the operating-system user as the
//! identity. This library exposes the pieces so that the test harness
//! (`mimlys-testloop`) drives the same code as the commands.

pub mod app;
pub mod commands;
pub mod config;
pub mod download;
pub mod follow;
pub mod logging;
pub mod messages;
pub mod output;
pub mod prompt;
pub mod storage;
pub mod toolchain;
pub mod workspace;
