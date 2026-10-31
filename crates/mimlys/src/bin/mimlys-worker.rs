//! `mimlys-worker`: the execution worker (REQ-ARCH-008). All of its logic
//! lives in `mimlys_exec::worker_main`; this binary exists so that
//! `cargo install mimlys` installs it beside `mimlys`.

fn main() {
    mimlys_exec::worker_main::main();
}
