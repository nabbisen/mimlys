# Handoff 011d — Acceptance and QA checklist (0.7.1)

- [ ] REQ-CONF-012: `io_uring_*`, `process_vm_*` denied always; the tests fail without the rules (mutation outputs attached).
- [ ] REQ-CONF-012: runtime and worker non-dumpable (their `/proc/<pid>` owned by root, `environ` unopenable from the same user); Landlock TCP layer on ABI ≥ 4, primitive `seccomp+landlock_net`.
- [ ] REQ-CONF-011: no `/proc` as a whole; the kept entries listed with reasons and none per-process; `/proc/<runtime pid>/environ` unreadable from a host tool.
- [ ] REQ-CONF-011 / LIM-H5: `~/.cargo`, the home directory, its ancestors and `/` refused with the narrower path suggested; `~/.cargo/bin` accepted; `toolchain grant` uses the same function.
- [ ] REQ-CONF-006: `effect` and `network` must agree (`external_read` exactly for `network = true`); refusals name the valid value; examples and CHANGELOG updated.
- [ ] REQ-TEST-013: the M6a suite and the demonstration green on Linux x86_64 and aarch64; Windows and macOS suites green on CI; platforms stated per suite.
- [ ] REQ-CONF-010: `scripts/check-unsafe.sh` green; each new `unsafe` block justified.
- [ ] Gate: fmt, clippy (Linux and `x86_64-pc-windows-gnu`), tests, `cargo deny`, the four scripts, `mdbook build docs`.
- [ ] Documents: ED §13.3 and §13.5 as amended, `host-tools.md`, `configuration.md`, `getting-started.md`, M6a report note, CHANGELOG `Unreleased` (Security).
- [ ] No contract change beyond the refusal; no version bump, tag or publish.
