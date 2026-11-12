# RFC 017 — Admit the worker and the interpreter modules by their files, not their directories

**Status.** Proposed (2026-11-12), for the owner's sign-off.
**Tracks.** Handoff 011f §3 (measured on `windows-latest`: an entry on `mimlys-worker.exe` alone, none on its directory, is enough for the container to start the worker and run tools); the replanning report §3 C ("entries without propagation"); the code at `main` (`crates/mimlys/src/app.rs`: `confinement_spec`, `fixed_read_paths`, and the runtime's own read paths for `toolchain::prepare`).
**Touches.** `mimlys` (`app.rs`, `toolchain.rs`), `mimlys-exec` (the three backends' handling of a read path that is a file; on Windows, `acl.rs`'s entry on a file and its record); requirements REQ-CONF-011 and REQ-CONF-008; external design §13.3, §13.5; threat register THR-36.
**Release.** 0.10.

## Summary

Today the read set of every worker, on every platform, holds **the directory** of the worker binary and the directory of every configured interpreter module. This RFC admits **the files** instead: the worker binary itself, and each module file. On Windows that turns mimlys's own permission write on a shared directory into one entry on one file, with no propagation. On every platform it removes what a host tool can read, and run, only because mimlys happens to be installed beside it.

## Motivation

- **Least privilege, today's gap.** mimlys installed by `cargo install` puts its worker in `~/.cargo/bin`, so every binary there is readable by every host tool. `fixed_read_paths` also makes any command under that directory pass the load check without a declaration. A configured interpreter module at `~/Downloads/qjs.wasm` puts all of `~/Downloads` in the read set.
- **A confusion, by the owner's rule.** What a tool may read and run then depends on where the user installed mimlys, not on what the user declared. The same configuration works after `cargo install` and fails after another install. Nothing tells the user why.
- **Windows' cost.** The worker's directory gets an inheritable entry, which Windows propagates to every file beneath it: 0.1 s to 9 s per write (D1). It is held under the permission lock, and every runtime of the machine contends for it on the shared directory. One entry on one file costs nothing and propagates nowhere. Handoff 011f measured that it is enough.

## Decision

1. **The worker binary is admitted as a file** in every worker's read set: Landlock on the file, a Seatbelt `literal`, and on Windows an entry on the file, not inheritable. Its directory is no longer in the read set, nor in `fixed_read_paths`.
2. **Each configured interpreter module is admitted as a file**, likewise. mimlys's own `interpreters` directory under the data directory stays: mimlys creates and owns it.
3. **Commands are judged against the declarations, not against where mimlys lives.** A host tool whose `command` lies beside the worker, such as `~/.cargo/bin/cargo`, now needs that directory in its `read_paths`. The existing refusal at load names the directory to declare (external design §13.5). The CHANGELOG lists this under Changed, with the remedy, and so does the book.
4. **Windows: a file entry is a first-class entry.** It is recorded in `toolchain-acls.json`, reported by `has_entry` and `toolchain list`, removed by `toolchain revoke` and `revoke --all`, and written under the permission lock like any other. 011f's measurement left these untested; the handoff tests each.
5. **What does not change.** mimlys's own directories under the data directory (`workspaces`, `tmp`, `interpreters`) keep their inheritable entries: mimlys creates them, so their propagation is over nearly empty trees. The user's declared toolchain directories keep theirs: a toolchain's files need the entry, and Windows checks the file, not its parents. Their write is the one-time cost of `toolchain grant`.

## Alternatives considered

- **Keep the directory and document it.** The gap and the confusion would stay, documented. Rejected under the owner's rule.
- **A file entry on Windows only.** It removes the cost but keeps the least-privilege gap on Linux and macOS, and one rule would mean different things per platform. Rejected.
- **Entries without propagation for the toolchain directories too.** Windows checks each file's own access list, so files that never inherited the entry are refused, unless traversal rights and per-file entries are managed separately. Rejected: the cost is paid once, at `toolchain grant`, by the user's choice.

## Consequences

- Host tools see less; the worker sees what it needs. On Windows, mimlys's own start no longer writes a propagating entry on a shared directory, so most of D1's contention disappears at its source.
- **A behaviour change for some users:** a configuration whose tool lies beside mimlys and relied on that, unknowingly, now gets a refusal at load naming the directory to declare. This is announced, and the remedy is one line.
- **What the handoff must measure before it lands:**
  - the worker needs nothing else from its directory, on all three platforms and under Rosetta;
  - an install whose directory is private (a user profile on Windows), not only the runners' layout;
  - the file entry's record and revocation on Windows.

## Open questions

None for the owner beyond acceptance.

## References

Handoff 011f §3 and its request; investigation D1; RFC 011 §7 and Amendment 1; requirements REQ-CONF-008, REQ-CONF-011; external design §13.3, §13.5; THR-36.
