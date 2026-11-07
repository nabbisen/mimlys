# Handoff 011c — Acceptance and QA checklist (M6c, 0.8)

- [ ] S1 reported and RFC 011 Amendment 2 written before any implementation.
- [ ] REQ-CONF-005 (macOS): Seatbelt self-applied before the first frame, canonical and escaped paths, `(deny default)`; each clause names its primitive; a missing or ineffective `sandbox_init` is `unavailable`, never a crash.
- [ ] REQ-CONF-011 (macOS): a host tool cannot read another process's information, `KERN_PROCARGS2` of the runtime included; the read set is the measured one, each entry with its reason.
- [ ] REQ-CONF-001: N refuses TCP to loopback and DNS; P holds against setuid exec, or is not claimed.
- [ ] K, M, C as Amendment 2 decides; no `RLIMIT_NPROC`; LIM-H6 used by name.
- [ ] RFC 011 §5: the tier and the `Tier` line of approvals as decided; `accept_best_effort` only where K is best-effort.
- [ ] REQ-CONF-010: every `unsafe` block in `confine/macos.rs` with its `SAFETY:` line; `check-unsafe.sh` green.
- [ ] REQ-TEST-013: the clause suite and the demonstration pass on `macos-latest`; mutation outputs attached; the Rosetta job reported apart as informative.
- [ ] M6c report, book, CHANGELOG; the gate; CI per platform with skip counts.
- [ ] (011c-5) A worker killed with `SIGKILL` leaves no member alive past LIM-O2 plus one poll; the sweeper's two mutations are caught.
- [ ] (011c-5) Two workers' listings do not see each other's members, or the slice stopped and reported.
- [ ] (011c-5) The count watchdog's tree ends `process_limit`, mapped to `resource_exhausted{processes}`; never `exited`.
- [ ] (011c-5) The LIM-H6, `xcrun_db` and Command Line Tools measurements are reported; nothing was added to the read set without the architect.
