#!/bin/sh
# The vacuity guard of the proof (REQ-TEST-018): maps a second `Held` to
# `Write` in the production file (mutation m1), runs the proof, and requires it
# to FAIL with an unmet postcondition, then restores the file and compares it
# byte for byte. A checker that silently proved everything would turn the CI
# job red here. From the project root:  sh verify/guard.sh
set -u
f=crates/mimlys-exec/src/confine/dacl_wait.rs
saved="${TMPDIR:-/tmp}/dacl_wait.saved.$$"
cp "$f" "$saved"
restore() { cp "$saved" "$f"; cmp "$f" "$saved" && rm -f "$saved"; }
trap restore EXIT

sed 's/Attempt::Held => Decision::Refuse(Cause::Held),/Attempt::Held => Decision::Write,/' "$saved" > "$f"
if cmp -s "$f" "$saved"; then
  echo "guard: the mutation did not apply (the text it maps has changed)" >&2
  exit 1
fi
out="$(sh verify/run.sh 2>&1)"
status=$?
echo "$out" | tail -n 25
if [ "$status" -eq 0 ]; then
  echo "guard: THE PROOF PASSED ON THE MUTATED FILE; the checker proves too much" >&2
  exit 1
fi
if ! echo "$out" | grep -q "postcondition not satisfied"; then
  echo "guard: the proof failed, but not on a postcondition" >&2
  exit 1
fi
echo "guard: the mutated file failed the proof, as required"
