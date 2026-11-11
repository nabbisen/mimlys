# verify/ — the Verus proof of the permission-lock decision

RFC 016, REQ-TEST-018. Not a workspace member, never packaged or published.
`dacl_wait.rs` here includes the production module,
`crates/mimlys-exec/src/confine/dacl_wait.rs`, with `#[path]`: the checker
reads the text that ships, never a copy. The specifications are in that file,
under `verus_keep_ghost`, which only Verus sets.

**Proved**, of `decide_from`: P1, a write only after an attempt that gave the
lock; P2, a lock held on both attempts refuses as held; P3, a failed attempt
that decides refuses as unavailable. Of `decide`, in part (P4) and **under a
hypothesis** (the `requires` on the closure, which no caller is verified
against): every call of the attempt passes `0` or the saturated limit, and the
one second call sits in the `Held` branch; `wait_ms` saturates.

**Not proved**, and held by the tests of the module: an attempt added outside
the `Held` branch, the order of the calls, their number. `decide` has no
postcondition. The caller in `windows::acl` and the mutex are outside it.
What was mutated and what survives is in
`docs/src/maintainers/repository-guards.md`.

Run, from the project root, with Verus 0.2026.11.02.8bf68ee on the `PATH` and
the Rust toolchain 1.99.1:

    sh verify/run.sh
