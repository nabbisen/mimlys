//! The mimlys command line and runtime host (RFC 010; external design §19).
//!
//! The binary embeds the runtime: one process per command, the data
//! directory locked while it runs, the operating-system user as the
//! identity. This library exposes the pieces so that the test harness
//! (`mimlys-testloop`) drives the same code as the commands.

// The output of the command line is written in `output` only, each write with its error handled,
// so that a closed pipe is no panic and no exit 101 (REQ-CLI-009). A `println!` or an
// `eprintln!` would panic on one, and a handle taken with `std::io::stdout()` or `stderr()`
// outside `output` and the prompt's terminal is refused by `clippy.toml`. The unit tests may
// print: a test binary is not the command.
#![cfg_attr(
    not(test),
    deny(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_methods)
)]
#![cfg_attr(
    test,
    allow(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_methods)
)]

pub mod app;
pub mod backends;
pub mod client;
pub mod commands;
pub mod config;
pub mod download;
pub mod embedded;
pub mod follow;
pub mod logging;
pub mod machine;
pub mod messages;
pub mod output;
pub mod probe;
pub mod probe_results;
pub mod prompt;
pub mod registry;
pub mod storage;
pub mod target;
pub mod toolchain;
pub mod workspace;
