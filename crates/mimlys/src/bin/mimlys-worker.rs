//! `mimlys-worker`: the execution worker (REQ-ARCH-008). All of its logic
//! lives in `mimlys_exec::worker_main`; this binary exists so that
//! `cargo install mimlys` installs it beside `mimlys`.

// Nothing of this binary prints (its streams belong to the runtime that spawns it, in
// `mimlys_exec::worker_main`): a print added here is refused, as in `mimlys` (`clippy.toml`).
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::disallowed_methods)]

fn main() {
    mimlys_exec::worker_main::main();
}
