# verify/ — Spike S3 (handoff 011k)

The Verus proof of the permission-lock decision, `decide` in
`crates/mimlys-exec/src/confine/dacl_wait.rs`. Not a workspace member, never
published. `dacl_wait.rs` here includes the production module with `#[path]`;
nothing is copied.

Run from the project root, with Verus 0.2026.11.02.8bf68ee on the `PATH`:

    sh verify/run.sh
