# Handoff 011f — Windows: the permission lock fails closed (release 0.8)

**RFC.** [RFC 011](../../accepted/011-confinement-backends.md) §7 (the container's entries on the user's directories) · **Specification.** REQ-CONF-008, external design §13.5 (the writers' lock), limits LIM-O7; threat THR-36 · **Decision.** Investigation D1 (`.git-exclude/review-requests/D1-windows-flake/README.md`), decided by the architect on 2026-11-07 (`e034744`) · **Release.** 0.8 · **Depends on** 011b, 011e (done).

## Corrections after the review of C9 (2026-11-07)

C9 (`c9-windows-lock`, PR #14) is approved in substance; three corrections on the same branch, then the request is updated at the top. The rules are in external design §13.5 and §18 and THR-36.

1. **No write without the lock.** A mutex the system will not create (`CreateMutexW` returns null) or wait on (`WaitForSingleObject` fails) refuses the write with its own reason: `acl_not_applied: the permissions lock of <dir> cannot be taken: <system error>`. `Decision::WriteUnlocked` goes; `Attempt::Failed` decides `Refuse` with that reason. Your question 1 is right that "being written by another runtime" would be a wrong reason; a write without the lock is the lost update THR-36 forbids. A second account's access to the lock is not added (the residual is in THR-36). Unit test and mutation check.
2. **The error kind is `unavailable`**, `detail.component = "permissions"`, for both refusals (external design §18; your question 2): the condition is expected and retrying is the remedy, and `internal` says "a runtime defect", which would mislead. The refusal travels as a typed error from `acl` to the command's error, never recognized by its text (no `starts_with("acl_not_applied:")`). The data directory's message carries the error's text in every case, not only its kind. The JSON test asserts the kind and the component.
3. **Book:** the `acl_not_applied` section names both reasons and says that retrying is the remedy.

Accepted as built: questions 3 to 6; the constant's name; `dacl_wait` in `mimlys-exec`; the abandoned mutex; section 3's measurement (recorded in the M6b report §4 as the basis of the replanning candidate "entries without propagation"). The throwaway branches stay until the owner decides on them.

## 1. Why

The writers of one directory's permissions hold a named mutex for the whole write. The mutex gave up after LIM-O2 and wrote anyway (fail-open), on the premise that a write takes milliseconds. D1 measured 0.1 s to 9 s, growing with the entries, because the system propagates the change to every file below. Each give-up wrote on a base read before the holder's write landed and removed another installation's entry; the user then saw `Access is denied` starting a tool, with no reason. The intermittent `m6b_host_tools` failure was this, and so would be two installations starting at once.

## 2. Behaviour (binding)

In `crates/mimlys-exec/src/confine/windows/acl.rs` (`DaclLock`):

- The mutex is held for the whole write, as now.
- A writer that finds the lock held writes **one** `info` log line naming the directory (`waiting for another runtime's write of the permissions of <dir>`), once per wait, then waits up to **LIM-O7** (new constant in the limits module, the doc comment naming `LIM-O7`). No other threshold: no number appears that the limits register does not hold.
- At the end of the wait it **refuses the write**: `acl_not_applied` with the reason `the permissions of <dir> are being written by another runtime`. Never a write on a base read before the holder's write landed.
- Who reports it: `toolchain grant` and the host tool's resolution report it as they report `acl_not_applied` today; at startup, mimlys's own directories (data directory, worker directory) make the start fail with the same reason (fail closed; the next start applies them).
- An abandoned mutex (its holder died) is acquired and the write proceeds; say so in the module header with the reason (the holder's write either landed or did not, and the writer re-reads the base under the lock).
- The module header no longer says "fails open"; it states the rule above and THR-36.

## 3. Measurement (report only)

Measure whether an entry on the **worker binary file** (`mimlys-worker.exe`), instead of its directory, is enough for the container to start the worker and the worker to run tools. If it is, mimlys's own write on the shared directory becomes one file, independent of the directory's contents. Report the result with the CI run id; do not change the entry. The decision is the architect's (a replanning candidate, "entries without propagation", depends on it).

## 4. Tests

- Unit (all platforms): the wait's decision as a pure function: lock free → write; held and released within the wait → write after it; held past the wait → `acl_not_applied` with the reason; abandoned → write. Names REQ-CONF-008.
- Windows (`tests/m6b_host_tools.rs`): `req_conf_008_concurrent_grants_keep_both_entries` — two installations (two data directories, two containers) grant the same directory at once; afterwards both containers' entries are present (`icacls`) and both run the probe tool.
- Evidence: D1's `ci-like` reproduction rerun once on the branch, ten iterations, expected 0 failures; the count in the request.
- Mutation check: LIM-O7 replaced by 1 ms; the concurrency test fails (or the refusal appears), and the output is in the request.

## 5. Documents

`docs/src/reference/host-tools.md` (the `acl_not_applied` section: the new reason and what to do: wait for the other runtime, retry); `docs/src/maintainers/m6b-report.md` only if the measurement of §3 changes a stated fact; CHANGELOG `Unreleased`. External design §13.5, THR-36 and LIM-O7 are already written (architect).

## 6. Return

`.git-exclude/review-requests/C9-windows-lock/README.md`: changed files, the gate as it ran, CI run ids per platform with skip counts, the reproduction's count, the mutation output, §3's measurement, differences from this handoff, limitations.
