# Handoff 011d — PR plan (0.7.1)

| PR | Content | Depends on | Size |
|---|---|---|---|
| 011d-1 | Probe-tool commands and the `probe.rs` helpers; the denied syscalls; tests with mutation checks | — | M |
| 011d-2 | `harden_process` in runtime and worker; the dumpable and cross-process tests | 011d-1 | S |
| 011d-3 | The Landlock network layer and the `seccomp+landlock_net` primitive; status and book tables | 011d-1 | S |
| 011d-4 | The `/proc` narrowing (method of §1.4) with the list and its reasons; the demonstration unchanged | 011d-2 | M |
| 011d-5 | LIM-H5 constant, the pure refusal function, configuration loading, `toolchain grant`; examples in the book | — | M |
| 011d-6 | `ubuntu-24.04-arm` in the CI matrix; CHANGELOG `Unreleased`; M6a report note; the review request | 011d-1 … 011d-5 | S |
