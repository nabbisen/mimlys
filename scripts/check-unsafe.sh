#!/usr/bin/env bash
# REQ-CONF-010 / REQ-ENG-001: foreign-function code and `unsafe` blocks live
# only under crates/mimlys-exec/src/confine/, each justified by a `SAFETY:`
# comment on the line before. Every other crate forbids `unsafe` through
# the workspace lints; this script keeps the exception honest.
set -euo pipefail
fail=0
offenders="$(grep -rln --include='*.rs' -E '(^|[^a-z_])unsafe( |\{|_)' crates | grep -v '^crates/mimlys-exec/src/confine/' || true)"
# `unsafe_code` lint attributes and the word in comments or docs are allowed; look for real uses.
real=""
for f in $offenders; do
  if grep -nE '^\s*(pub )?unsafe (fn|impl|\{)|[^a-z_]unsafe \{' "$f" | grep -vE '^\s*[0-9]+:\s*//' >/dev/null; then real="$real $f"; fi
done
if [ -n "$real" ]; then echo "unsafe outside crates/mimlys-exec/src/confine/:$real" >&2; fail=1; fi
# inside confine/, every unsafe block or impl carries a SAFETY justification
while IFS= read -r f; do
  awk -v file="$f" '
    /^[[:space:]]*\/\/ SAFETY:/ { ok = NR }
    /unsafe (\{|impl)/ && !/^[[:space:]]*\/\// { if (ok != NR - 1 && ok != NR - 2) { printf "%s:%d: unsafe without a SAFETY comment\n", file, NR; bad = 1 } }
    END { exit bad }' "$f" || fail=1
done < <(grep -rl --include='*.rs' 'unsafe' crates/mimlys-exec/src/confine/ || true)
if [ "$fail" -ne 0 ]; then exit 1; fi
echo "ok: unsafe confined to crates/mimlys-exec/src/confine/ with SAFETY justifications"
