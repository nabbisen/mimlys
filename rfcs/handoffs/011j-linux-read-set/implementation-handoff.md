# Handoff 011j — Linux: narrow `/etc` and `/opt` in the host tools' read set (release 0.10)

**RFC.** [RFC 011](../../done/011-confinement-backends.md) (Amendment 1: the read set from measurement) · **Specification.** REQ-CONF-011, external design §13.3 (the Linux read set); threat register: the residual "`/etc` and `/opt` are readable as a whole" (since 0.7.1).

The Linux read set includes `/etc` and `/opt` as wholes (`crates/mimlys/src/app.rs`). Narrow them the way handoff 011d narrowed `/proc`, by measurement:
- run the full suite and the demonstration (`cargo test --offline` of the native project, and the dependency step) under a read set without `/etc` and `/opt`;
- add back the single files or directories that a failure asks for, **one at a time, each with the failure that asked for it** in the request (candidates to expect: `ld.so.cache`, `ld.so.conf*`, `localtime`, `nsswitch.conf`, `passwd`, `group`, the certificate files for the dependency step only);
- `/opt` is not in the read set unless a declaration names a path under it.

Measure on `ubuntu-latest` and `ubuntu-24.04-arm` (both run the M6a suite) and on a Debian- and an Arch-family layout if available (say which ran). The external design's Linux read-set paragraph and the threat register's residual are mine to rewrite from your list.

Return: `.git-exclude/review-requests/011j-linux-read-set/README.md`: the final list, each entry's failure, the mutation (one entry removed, its test fails), CI with skips.
