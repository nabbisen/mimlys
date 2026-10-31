#!/usr/bin/env bash
# REQ-ARCH-002: mimlys-core must not depend on an engine, a store, a transport,
# a protocol SDK or a CLI framework. Run from the repository root.
set -euo pipefail
forbidden='^(wasmtime|wasmtime-wasi|wasmtime-wasi-http|rusqlite|sqlx|libsqlite3-sys|reqwest|hyper|hyper-util|rmcp|mcp|clap)$'
deps="$(cargo tree -p mimlys-core -e normal --prefix none | awk '{print $1}' | sort -u)"
if printf '%s\n' "$deps" | grep -Eq "$forbidden"; then
  echo "mimlys-core depends on a forbidden crate:" >&2
  printf '%s\n' "$deps" | grep -E "$forbidden" >&2
  exit 1
fi
echo "ok: mimlys-core has no forbidden dependencies"
