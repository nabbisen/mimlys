# M6b report: the Windows backend

Milestone M6b (RFC 011 §3, §7; handoff 011b) was implemented on 2026-10-31
on a Linux machine against the platform's documentation, type-checked for
`x86_64-pc-windows-gnu`, and is gated by the Windows runner of CI
(`windows-latest`), where the clause suite `tests/m6b_host_tools.rs` and the
demonstration run. This report is completed from that run.

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

*Filled from the first CI run on Windows.*

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
