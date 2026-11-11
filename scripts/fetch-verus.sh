#!/usr/bin/env bash
# Fetches the pinned Verus release to DEST for the CI job that checks the
# permission-lock proof (RFC 016, REQ-TEST-018; the download rules of handoff
# 005b): retried on a transient failure (`--retry-all-errors`, up to 8 times
# with curl's exponential backoff), each attempt bounded by 120 s, verified
# against the pinned SHA-256 **before** anything is unpacked (a mismatch
# fails), then unpacked so that DEST holds `verus`, `rust_verify`, `z3` and
# the rest of the release. The version and the hash are written here and
# nowhere else: the CI cache is keyed by this file, so a Verus upgrade (its own
# change, with the Rust toolchain it names) is a change to these three lines.
# Run from the repository root; needs `curl`, `unzip` and `sha256sum`.
set -euo pipefail

release="0.2026.11.02.8bf68ee"
archive="verus-${release}-x86-linux.zip"
want="f3a0119292df8855e4435da8130bee1873b0e6a60c306accf81a08ea687bba3c"
url="https://github.com/verus-lang/verus/releases/download/release/${release}/${archive}"

dest="${1:?usage: fetch-verus.sh DEST}"
part="${dest}.zip.part"
rm -rf "$dest" "$part"
mkdir -p "$(dirname "$dest")"
curl --fail --silent --show-error --location \
  --retry 8 --retry-all-errors --retry-max-time 600 --max-time 120 \
  --output "$part" "$url"
got="$(sha256sum < "$part" | cut -d' ' -f1)"
if [ "$got" != "$want" ]; then
  rm -f "$part"
  echo "fetch-verus: $url has SHA-256 $got, the pin is $want" >&2
  exit 1
fi
unpack="${dest}.unpack"
rm -rf "$unpack"
mkdir -p "$unpack"
unzip -q "$part" -d "$unpack"
rm -f "$part"
# The archive holds one directory (`verus-x86-linux`); DEST is that directory.
inner="$(dirname "$(find "$unpack" -name verus -type f | head -n1)")"
mv "$inner" "$dest"
rm -rf "$unpack"
chmod +x "$dest/verus" "$dest/rust_verify" "$dest/z3"
echo "fetch-verus: $dest verified ($release, $want)"
