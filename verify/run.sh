#!/bin/sh
# Spike S3 (handoff 011k): checks the proof of the permission-lock decision
# (crates/mimlys-exec/src/confine/dacl_wait.rs) with the installed Verus.
# Run from the project root: sh verify/run.sh
set -e
cargo build -p mimlys-core --locked
cd verify
verus dacl_wait.rs "$@" \
  -L dependency=../target/debug/deps \
  --extern mimlys_core=../target/debug/libmimlys_core.rlib
