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
tag with upstream's wasi-sdk version, so that a reviewer can compare the hash:

```sh
docker build -t mimlys-qjs tools/interpreters/quickjs
docker run --rm mimlys-qjs sha256sum /out/qjs-wasi.wasm
```

The recipe repeats upstream's own release job (wasi-sdk 29 `.deb`, `cmake` with
the wasi-sdk toolchain file and `-DQJS_BUILD_WERROR=ON`, `make -C build
qjs_exe`) and reproduces the pinned artifact byte for byte. The `check` in `.github/workflows/interpreters.yml`
rebuilds on demand and reports whether the hash matches `expected.sha256`.
A mismatch is a review item, not a release blocker: the pinned artifact stays
what the binary verifies.

## What was wrong with the recipe (2026-11-03)

The scheduled check failed from its first run (rebuilt `cd7da6cb…`, pin `42a732a6…`). The pin was never at fault: the `v0.17.0` release asset `qjs-wasi.wasm` is 1 569 050 bytes with that SHA-256. The recipe differed from upstream's `wasi` job at that tag (`.github/workflows/release.yml`, lines 142-154) in four places, and was aligned with it:

| | Old recipe | Upstream job, now the recipe |
|---|---|---|
| wasi-sdk | 25, tarball | 29, the `.deb` upstream installs (unpacks to `/opt/wasi-sdk`) |
| Build type | `-DCMAKE_BUILD_TYPE=Release` | not set (upstream's `CMakeLists.txt` defaults to Release, so no effect on the output) |
| Warnings | not set | `-DQJS_BUILD_WERROR=ON` |
| Build command | `cmake --build --target qjs_exe` | `make -C build qjs_exe` |

Result: with the aligned recipe the `interpreters` workflow rebuilds `42a732a6…` (1 569 050 bytes), equal to the pin and to `expected.sha256` (run 41601316708 on branch `t3-interpreter-recipe`). Building the aligned recipe locally with podman and `--build-arg WASI_SDK_VERSION=25` gives `cd7da6cb…` again, so the wasi-sdk version is what changed the bytes; the other three differences did not.

## Inputs from outside (pinned 2026-11-03, T3b)

| Input | How it is pinned | Checked against |
|---|---|---|
| base image | digest of `debian:bookworm-slim` | (unchanged) |
| wasi-sdk 29 `.deb` | SHA-256 `20a5b880814adc14b93f8b14a2230e75a6174a1b77493c50e8fc00d27dcfdcb2` (82 995 002 bytes), verified by `sha256sum -c` before it is installed | the digest GitHub shows for the release asset, and two separate downloads: all three equal. The wasi-sdk release publishes no checksum file |
| QuickJS-NG source | commit `6d46d07d04041b40f4f49eaa7fdebe44c314c699` (`git clone`, then `git checkout --detach`), the commit tag `v0.17.0` points at | `git ls-remote` of the tag and GitHub's API for `v0.17.0`: the same commit |
| apt packages (`cmake`, `make`, `git`, `curl`) | **not pinned**, deliberately | the base image is pinned, so the set is fixed per image; and the output does not depend on them: upstream's release job builds on `ubuntu-22.04` (cmake 3.22) and this recipe on bookworm (cmake 3.25), and both give `42a732a6…` |

To update wasi-sdk or the source, change the version, the SHA-256 and the commit together, and re-run the `interpreters` workflow: a build with a version whose digest is not given fails at `sha256sum -c`.

## Updating

1. Pick the new upstream release; download `qjs-wasi.wasm`; compute its SHA-256.
2. Change `PINNED` (version, URL, hash) and `expected.sha256`; update this file.
3. Run the M2 demonstration tests (`cargo test -p mimlys --test m2_demo`) and the
   `interpreters` workflow; record both in the changelog under "Changed"
   (REQ-ARCH-007: a reviewed change that names what the update fixes and gains).
