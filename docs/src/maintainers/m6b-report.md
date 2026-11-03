# M6b report: the Windows backend

Milestone M6b (RFC 011 §3, §7; handoff 011b) was implemented on 2026-10-31
on a Linux machine against the platform's documentation, type-checked for
`x86_64-pc-windows-gnu`, and is gated by the Windows runner of CI
(`windows-latest`), where the clause suite `tests/m6b_host_tools.rs` and the
demonstration run. Its tables are those of the CI runs of 2026-11-03 (§2: run
41552997121; the suite's final result: runs 41566294345 and 41568150855,
all seven clause tests passing; the `cargo` demonstration is stated as not
met, §4).

## 1. The backend

| Clause | Primitive | How |
|---|---|---|
| F | `appcontainer` | one AppContainer per installation (`mimlys-<hash>`); the container SID holds entries on the workspaces and `<data>/tmp` (full), the interpreters and the worker's directory (read), applied at startup and recorded; the toolchain paths by `toolchain grant` |
| N | `appcontainer` | no network capability; the dependency step's worker carries `internetClient` |
| E | `explicit_environment` | the worker's and every host tool's environment built from nothing; the profile paths inside the workspace's temporary directory |
| K | `job_object` | the worker in a job with kill-on-close and breakaway forbidden, joined while suspended; every host tool in a nested job of its own, killed as one on cancel or deadline |
| P | `restricted_token+appcontainer`, else `appcontainer` | a restricted (`DISABLE_MAX_PRIVILEGE`) low-integrity primary token with the container where `CreateProcessAsUser` accepts the pair; otherwise the container's lowbox token, which has no privileges and cannot elevate |
| M | `job_object` | `JobMemoryLimit` = LIM-H2 |
| C | `job_object` | `ActiveProcessLimit` = LIM-H3 |

The worker reports every clause from its own token (`TokenIsAppContainer`,
`TokenCapabilities`, `TokenIntegrityLevel`, `IsTokenRestricted`) and its own
job (`QueryInformationJobObject`), never from what the parent intended.

## 2. The clause table on `windows-latest`

From CI run 41552997121 on `windows-latest` (2026-11-03, commit `30fa57a`),
the first run on which the worker's pipes carried its answer (the parent's
ends had been wrapped in the standard library's child-stream types, whose
alertable overlapped I/O stalls on an anonymous pipe handle; they are
synchronous files since that commit). `mimlys status` reported tier
**full**:

| Clause | State | Primitive | Note |
|---|---|---|---|
| F filesystem | enforced | `appcontainer` | |
| N network | enforced | `appcontainer` | no capability on the offline worker |
| E environment | enforced | `explicit_environment` | |
| K termination | enforced | `job_object` | |
| P privilege | enforced | `appcontainer` | `CreateProcessAsUserW` refuses the restricted low-integrity token together with the container on this runner, so the worker runs under the container's lowbox token alone, which the code accepts as P (no privileges, no elevation) and reports as such |
| M memory | enforced | `job_object` | |
| C count | enforced | `job_object` | |

The M1 to M6a suites passed on the same run. The M6b suite's own
expectations were corrected in the following rounds (the status JSON's host-tool
objects, the verbatim prefix of a path argument, the short-form temporary
directory, the DACL write); from run 41566294345 (2026-11-03) every suite,
the seven clause tests included, passes on all three platforms.

## 3. What the clause tests check (REQ-TEST-013)

| Try | Expected |
|---|---|
| read a file beside the worker's directory | `read failed` (no entry for the container) |
| write outside the workspace | `write failed`; inside: written |
| connect to a loopback listener | `connect failed` (no capability; loopback is closed to containers) |
| read the operator's `MIMLYS_TEST_SECRET` | unset; the declared `PROBE_OK` present |
| start a detached child in a new process group, then the deadline | the tool `deadline`, the child gone with the job |
| spawn 100 | the job's `ActiveProcessLimit` stops it; non-zero exit |
| allocate 3 000 MiB | the job's memory limit ends it; non-zero exit |
| `MIMLYS_PROBE_FAIL=F` | tier `core`, `run_host_tool` unlisted |
| `--all`, `../x`, four arguments, an undeclared name | each `denied` |
| a `network = true` tool | `external_read`; the offline sibling cannot connect |
| a declared path without its entry | `acl_not_applied`, the grant refused; `toolchain grant --yes` applies, `revoke --yes` leaves `icacls` output identical |
| the demonstration | `cargo test --offline` of a Cargo workspace with a path dependency, `completed{verified}`, after `toolchain grant` on `.cargo` and `.rustup` |

## 4. Known platform facts

- An AppContainer cannot reach loopback without an administrator's
  exemption (`CheckNetIsolation LoopbackExempt`); the dependency step is for
  the network, not for local servers.
- The worker binary's directory must carry the container's read entry; a
  `cargo install` location under the profile receives it at startup, recorded
  as a runtime entry that `revoke` refuses.
- There is no orphan to reap: the job's kill-on-close ends the worker's tree
  when the runtime's handle closes, a crash included.
- An AppContainer cannot open the `NUL` device on `windows-latest` (Windows
  Server 2025): `cargo` opens it for every `rustc` it starts and fails at
  `rustc -vV` with `Access is denied`. The demonstration of §23 is therefore
  not claimed on Windows in 0.7; the suite states the fact when it meets it.
  A spike on `windows-2022` and `windows-latest` (2026-11-03) found the same
  refusal on both images, for `NUL`, `\\.\NUL` and `\\?\GLOBALROOT\Device\Null`; a
  child started with a piped or inherited standard input runs, one with a null
  input does not. No way to lend the device that keeps clauses F, N and P was
  found: an exception without a remedy.
- Two installations writing one directory's DACL at once (both grant their
  container the worker's directory at startup) lost one another's entry,
  and a tool's command then failed to resolve inside the loser's container
  with no reason recorded; the writers hold a named mutex per directory
  since round 11, and a tool that does not start carries its reason. The
  DACL merge starts from the explicit entries alone and the write names the
  directory's protection flag, so inherited entries stay inherited and
  `revoke` leaves the directory's entries as they were. One bit differs: the
  write marks the descriptor auto-inherited (`SE_DACL_AUTO_INHERITED`), as
  every `SetNamedSecurityInfo` call does, after which `icacls` prints `(I)`
  on the inherited entries; a directory the shell created shows them without
  the marker although their inherited flag is set. The suite compares the
  entries without the marker.
- The epoch ticker owes ticks after a stretched sleep and delivers them at
  once (a 100 ms deadline ended at 76 ms on `macos-latest`); a deadline is
  armed past the owed ticks, so an execution never ends before its deadline.
