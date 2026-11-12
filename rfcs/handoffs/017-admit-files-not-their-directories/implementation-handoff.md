# Handoff 017 — The worker and the interpreter modules admitted by their files (release 0.10)

**RFC.** [RFC 017](../../accepted/017-admit-files-not-their-directories.md) (accepted 2026-11-12) · **Specification.** REQ-CONF-011 and REQ-CONF-008 (amended 2026-11-12); external design §13.3, §13.5; THR-36 · **From.** Handoff 011f §3 (an entry on `mimlys-worker.exe` alone sufficed on `windows-latest`).

## 1. Behaviour (binding)

- **`crates/mimlys/src/app.rs`.** In `confinement_spec`, in `fixed_read_paths`, and in the runtime's read paths passed to `toolchain::prepare`:
  - the worker binary's **file** replaces its parent directory;
  - each configured interpreter module's **file** replaces its parent directory;
  - mimlys's own `<data>/interpreters` stays, as a directory.
- **Each backend admits a read path that is a file:**
  - Linux: Landlock on the file, with the read and execute rights that apply to a file;
  - macOS: a Seatbelt `literal` (the generator already writes files as `subpath`, which matches the file and nothing under it: keep whichever is exact, and say which);
  - Windows: an entry on the file, **not** inheritable.
- **Windows, the file entry is first-class (REQ-CONF-008):** recorded in `toolchain-acls.json`, reported by `has_entry` and `toolchain list`, removed by `toolchain revoke` and `revoke --all` (its scan includes the worker file), and written under the permission lock.
- **The load check (external design §13.5)** judges a `command` against the fixed read paths, which no longer include the worker's directory, and the tool's own `read_paths`. A tool whose command lies beside mimlys, such as `~/.cargo/bin/cargo` with no `~/.cargo/bin` declared, is refused at load, naming the directory to declare.

## 2. Measure before it lands

Each with the evidence in the request:
1. The worker needs nothing else from its directory: the full suite and the demonstrations on Linux x86_64 and aarch64, macOS arm64, Rosetta and Windows, with only the file admitted.
2. On Windows, an install whose directory is private to the user, not the runner's `target\debug`: for example, the worker copied into a folder under the user's profile, with no entry on the folder.
3. The file entry's record, `has_entry`, `toolchain list`, `revoke` and `revoke --all` on Windows.
4. On Windows, the time of mimlys's own start before and after, on the shared directory of the test suite: the write that D1 measured at 0.1 s to 9 s.

## 3. Tests (each names its requirement; mutations)

- The read set holds the worker's file and not its directory, and each module's file and not its directory (all platforms; a test also builds a spec with a module in a folder holding other files, and those are refused inside confinement on each backend).
- A command beside the worker without a declaration is refused at load, naming the directory; with the declaration it loads.
- **Windows:** the file entry is recorded, listed and revoked; `revoke --all` leaves the file's access list as it was before mimlys.
- **Mutations:** the directory put back; the module's directory put back; the Windows entry made inheritable on a directory; `revoke --all` skipping the file.

## 4. Documents

- `docs/src/reference/host-tools.md`: the read set's words (lines saying "the worker's own directory"), and a paragraph on the change and its remedy.
- The CHANGELOG under Changed, with the remedy; the configuration chapter where the worker's directory is mentioned.
- The M6b report's line on the worker's directory stays as history, with one added sentence pointing to RFC 017.

## 5. Return

`.git-exclude/review-requests/017-admit-files/README.md`: the change, the measurements of §2, the gate, CI per platform with skips, the mutations.
