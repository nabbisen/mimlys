# QuickJS-NG, wasip1 build (the `javascript` interpreter)

mimlys ships the upstream release artifact of QuickJS-NG, pinned by version
and SHA-256 in `crates/mimlys-exec/src/interpreters.rs` (`PINNED`):

| Field | Value |
|---|---|
| Upstream | https://github.com/quickjs-ng/quickjs, release `v0.17.0` (2026-09-18) |
| Artifact | `qjs-wasi.wasm` (1 569 050 bytes, wasip1 command module, `_start`) |
| SHA-256 | `42a732a676ec2d93488c19411e0fad283bf72658fdad746f089914b523c783b1` |
| Script entry | `/tmp/main.js`; `std` and `os` modules are not part of this build (scripts read stdin and args and print; files are the file tools' job) |

`mimlys interpreters install javascript` downloads the artifact, verifies the
hash and stores it under `<data_dir>/interpreters/<sha256>.wasm`. A module
whose hash differs is never loaded (REQ-SBX-007, REQ-SBX-012).

## Reproducing the build

`Dockerfile` in this directory rebuilds the artifact from the pinned upstream
tag with the pinned wasi-sdk, so that a reviewer can compare the hash:

```sh
docker build -t mimlys-qjs tools/interpreters/quickjs
docker run --rm mimlys-qjs sha256sum /out/qjs-wasi.wasm
```

The recipe follows upstream's own release workflow (`cmake` with the wasi-sdk
toolchain file, target `qjs_exe`). Byte-for-byte reproducibility depends on
upstream's toolchain pin; the `check` in `.github/workflows/interpreters.yml`
rebuilds on demand and reports whether the hash matches `expected.sha256`.
A mismatch is a review item, not a release blocker: the pinned artifact stays
what the binary verifies.

## Known differences from upstream's release job (2026-11-03)

The scheduled check has failed since its first run (rebuilt `cd7da6cb…`, pin `42a732a6…`). The pin is authentic: the `v0.17.0` release asset `qjs-wasi.wasm` is 1 569 050 bytes with that SHA-256. The recipe differs from upstream's `wasi` job at that tag (`.github/workflows/release.yml`) in four places: wasi-sdk 25 (upstream 29), `-DCMAKE_BUILD_TYPE=Release` (upstream: the default), no `-DQJS_BUILD_WERROR=ON` (upstream: on), and `cmake --build --target qjs_exe` on Debian bookworm (upstream: `make -C build qjs_exe` on ubuntu-22.04). The recipe is to be aligned with the job; the result of that run is recorded here when it exists. The pin does not change on the strength of a mismatch.

## Updating

1. Pick the new upstream release; download `qjs-wasi.wasm`; compute its SHA-256.
2. Change `PINNED` (version, URL, hash) and `expected.sha256`; update this file.
3. Run the M2 demonstration tests (`cargo test -p mimlys --test m2_demo`) and the
   `interpreters` workflow; record both in the changelog under "Changed"
   (REQ-ARCH-007: a reviewed change that names what the update fixes and gains).
