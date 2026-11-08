#!/usr/bin/env bash
# Prints one field (`url` or `sha256`) of the pinned JavaScript interpreter
# artifact. The single source is crates/mimlys-exec/src/interpreters.rs, so
# that the CI and the product cannot disagree about which artifact is pinned.
# Run from the repository root.
set -euo pipefail
field="${1:?usage: pinned-interpreter.sh url|sha256}"
case "$field" in url | sha256) ;; *) echo "unknown field: $field" >&2; exit 2 ;; esac
value="$(sed -n "s/^ *$field: \"\\(.*\\)\",\$/\\1/p" crates/mimlys-exec/src/interpreters.rs | head -n 1)"
[ -n "$value" ] || { echo "no pinned $field found" >&2; exit 1; }
printf '%s\n' "$value"
