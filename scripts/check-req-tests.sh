#!/usr/bin/env bash
# REQ-TEST-001, REQ-SEC-004: lists every requirement marked `T` that no test
# names, and every invariant INV-01 … INV-14 that no test names. A test names
# a requirement by its identifier in its function name (`req_task_002_…`) or
# in its doc comment (`REQ-TASK-002`); the search covers the test files only
# (`tests.rs` beside a module, and everything under a crate's `tests/`).
#
# The invariants line FAILS (docs/src/maintainers/repository-guards.md §3): an
# invariant that no test names exits 1 (REQ-SEC-004; the `rules` job). The
# requirements list is REPORT ONLY: it prints and exits 0; with `--strict` it
# also fails when that list is not empty (the switch for the day it is). A
# malformed `scripts/req-test-map.txt` exits 1 as well.
#
# `scripts/req-test-map.txt` holds the requirements met structurally, one per
# line, `ID reason`; a mapped requirement is not listed. A line without a
# reason, or naming an identifier that is not a `T` requirement or an
# invariant, is an error. No test is written to empty the list: a test that
# names a requirement it does not verify is worse than a listed gap.
#
# Run from the repository root.
set -euo pipefail

strict=0
[ "${1:-}" = "--strict" ] && strict=1

requirements='docs/src/maintainers/requirements.md'
design='docs/src/maintainers/external-design.md'
map='scripts/req-test-map.txt'

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Requirements verified by test: `### REQ-XXX-nnn Title — M1 · T` (the codes
# after the dot may be several: `T, D`).
grep -E '^### REQ-[A-Z]+-[0-9]{3} ' "$requirements" \
  | awk '{ id = $2; line = $0; sub(/^.*· */, "", line); n = split(line, codes, /[ ,]+/);
           for (i = 1; i <= n; i++) if (codes[i] == "T") { print id; break } }' \
  | sort -u > "$tmp/t-requirements"
grep -oE '^\| INV-[0-9]{2} ' "$design" | sed -E 's/^\| //; s/ $//' | sort -u > "$tmp/invariants"

# The mapping file: `ID reason`, comments and blank lines skipped.
: > "$tmp/mapped"
bad=0
if [ -f "$map" ]; then
  n=0
  while IFS= read -r line || [ -n "$line" ]; do
    n=$((n + 1))
    case "$line" in ''|'#'*) continue ;; esac
    id="${line%%[[:space:]]*}"
    reason="${line#"$id"}"
    reason="${reason#"${reason%%[![:space:]]*}"}"
    if [ -z "$reason" ]; then
      echo "$map:$n: $id has no reason" >&2; bad=1; continue
    fi
    if ! grep -qxF "$id" "$tmp/t-requirements" "$tmp/invariants"; then
      echo "$map:$n: $id is neither a requirement marked T nor an invariant" >&2; bad=1; continue
    fi
    echo "$id" >> "$tmp/mapped"
  done < "$map"
fi
sort -u "$tmp/mapped" -o "$tmp/mapped"

# The test corpus, lowercased with hyphens as underscores, so that
# `REQ-TASK-002` and `req_task_002_…` are one spelling.
find crates -type f -name '*.rs' \( -name tests.rs -o -path '*/tests/*' \) -print0 \
  | sort -z | xargs -0 cat | tr 'A-Z' 'a-z' | tr '-' '_' > "$tmp/corpus"

named() { # is the identifier named in the corpus?
  local spelled
  spelled="$(printf '%s' "$1" | tr 'A-Z' 'a-z' | tr '-' '_')"
  grep -qE "(^|[^a-z0-9])${spelled}([^0-9]|$)" "$tmp/corpus"
}

report() { # report <title> <file of identifiers>
  local total unnamed=0 mapped=0
  total="$(wc -l < "$2")"
  : > "$tmp/unnamed"
  while IFS= read -r id; do
    if grep -qxF "$id" "$tmp/mapped"; then mapped=$((mapped + 1)); continue; fi
    if ! named "$id"; then echo "$id" >> "$tmp/unnamed"; unnamed=$((unnamed + 1)); fi
  done < "$2"
  echo "$1: $unnamed of $total named by no test ($mapped met structurally, see $map)"
  sed 's/^/  /' "$tmp/unnamed"
  count_unnamed=$((count_unnamed + unnamed))
}

count_unnamed=0
report "requirements marked T" "$tmp/t-requirements"
requirements_unnamed=$count_unnamed
count_unnamed=0
report "invariants" "$tmp/invariants"
invariants_unnamed=$count_unnamed

if [ "$bad" -ne 0 ]; then exit 1; fi
if [ "$invariants_unnamed" -ne 0 ]; then
  echo "FAIL: $invariants_unnamed invariant(s) named by no test (REQ-SEC-004)" >&2
  exit 1
fi
if [ "$strict" -eq 1 ] && [ "$requirements_unnamed" -ne 0 ]; then exit 1; fi
exit 0
