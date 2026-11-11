#!/bin/sh
# Checks the Verus proof of the permission-lock decision
# (crates/mimlys-exec/src/confine/dacl_wait.rs; RFC 016, REQ-TEST-018). Needs
# Verus 0.2026.11.02.8bf68ee on the PATH and the Rust toolchain 1.99.1 (the one
# Verus names; `mimlys-core` is built with it). From the project root:
#   sh verify/run.sh [extra verus options]
set -e
cargo build -p mimlys-core --locked
cd verify
verus dacl_wait.rs "$@" \
  -L dependency=../target/debug/deps \
  --extern mimlys_core=../target/debug/libmimlys_core.rlib
