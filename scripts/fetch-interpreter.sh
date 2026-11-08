#!/usr/bin/env bash
# Fetches the pinned JavaScript interpreter artifact to DEST for the CI (handoff
# 005b): the download is retried on a transient failure (`--retry-all-errors`:
# connection errors, timeouts, 429, 5xx) up to 8 times with curl's exponential
# backoff (1 s doubling: about four minutes in all, never more than ten),
# each attempt bounded by 120 s, and verified against the
# pinned SHA-256 before DEST exists. The tests then use the file through
# MIMLYS_QJS_WASM instead of downloading it themselves. Run from the
# repository root.
set -euo pipefail
dest="${1:?usage: fetch-interpreter.sh DEST}"
url="$(scripts/pinned-interpreter.sh url)"
want="$(scripts/pinned-interpreter.sh sha256)"
mkdir -p "$(dirname "$dest")"
part="$dest.part"
rm -f "$part"
curl --fail --silent --show-error --location \
  --retry 8 --retry-all-errors --retry-max-time 600 --max-time 120 \
  --output "$part" "$url"
if command -v sha256sum > /dev/null 2>&1; then
  got="$(sha256sum "$part" | cut -d' ' -f1)"
else
  got="$(shasum -a 256 "$part" | cut -d' ' -f1)"
fi
if [ "$got" != "$want" ]; then
  rm -f "$part"
  echo "fetch-interpreter: $url has SHA-256 $got, the pin is $want" >&2
  exit 1
fi
mv "$part" "$dest"
echo "fetch-interpreter: $dest verified ($want)"
