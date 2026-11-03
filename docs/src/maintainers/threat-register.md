# Threat Register

**Status:** Baseline 1.0 (2026-10-02). A living document: reviewed at every milestone and whenever a data flow, an external integration or an authority mechanism changes (REQ-SEC-001; project rule "Release Deliverables").

Boundaries B1 to B9 are defined in requirements §3. Every threat names its controls as requirements and the tests that verify them. "Tests of REQ-x" means the tests that requirement carries itself.

| ID | Threat | Boundary | Impact | Controls | Verified by |
|---|---|---|---|---|---|
| THR-01 | Prompt injection: workspace content, tool results, documents or answers carry text meant to steer the model | B3, B6 | Unsafe proposal | REQ-CTX-002, REQ-SEC-002, REQ-CAP-001, REQ-APR-001 | REQ-TEST-010, REQ-TEST-017 |
| THR-02 | Malformed or oversized model output | B2 | Parser hazard; silent misinterpretation | REQ-DEC-003, REQ-SEC-009, REQ-MDL-007 | REQ-TEST-004 |
| THR-03 | Unauthorized filesystem access through an action: traversal, symbolic links, case tricks, reserved names, existence probing | B3 | Read or write outside scope; information leak | REQ-DEC-007, REQ-TOOL-004, REQ-WS-011, REQ-DEC-008 | REQ-TEST-005 |
| THR-04 | Infinite or excessive computation in generated code | B5 | CPU exhaustion; a stuck task | REQ-SBX-002, REQ-TASK-009 | REQ-TEST-008 |
| THR-05 | Memory exhaustion in generated code | B5 | Host instability | REQ-SBX-002, REQ-SCHED-003 | REQ-TEST-008 |
| THR-06 | Output flood | B4, B5 | Context and storage exhaustion | REQ-SBX-005, REQ-TOOL-008, REQ-PERS-005 | REQ-TEST-008 |
| THR-07 | Self-correction pollution: a failed attempt damages unrelated files | B8 | Unrecoverable workspace | REQ-TOOL-010, REQ-WS-004, REQ-WS-006 | REQ-TEST-009 |
| THR-08 | Replay of an irreversible action after a crash | B7 | Duplicate external effect | REQ-PERS-004, REQ-PERS-007, REQ-PERS-008 | REQ-TEST-007 |
| THR-09 | Capability escalation through model text, plan revision or tool chaining | B2 | Broader permissions | REQ-CAP-001, REQ-CAP-006, REQ-SEC-002 | REQ-TEST-006, REQ-TEST-017 |
| THR-10 | Malicious tool output steering the model | B3, B6 | Unsafe proposal | REQ-CTX-002, REQ-CTX-009, REQ-MCP-006 | REQ-TEST-010, REQ-TEST-012 |
| THR-11 | Network exfiltration from generated code or the worker | B5 | Data leaves the host | REQ-SBX-004, REQ-SEC-006, REQ-CONF-002 | REQ-TEST-008, REQ-TEST-013 |
| THR-12 | Supply chain: a vulnerable dependency, engine drift, a hostile interpreter build | — | Any of the above | REQ-SEC-012, REQ-ARCH-007, REQ-SBX-007 | REQ-TEST-014 |
| THR-13 | Policy misconfiguration: a ceiling or profile that grants too much | B9 | Excess authority | REQ-CFG-001, REQ-CFG-004, REQ-CAP-009, REQ-SEC-014 | tests of REQ-CFG-001 |
| THR-14 | Compromise of the runtime process or the operating system | — | Everything | Out of scope beyond REQ-SEC-008, REQ-CONF-002 and REQ-ARCH-008 | accepted residual |
| THR-15 | Task flooding and scheduler exhaustion | B1 | Resource consumption | REQ-SCHED-001, REQ-TASK-012 | REQ-TEST-011 |
| THR-16 | Sensitive data exposure: secrets or private files reach the model or a remote backend | B2, B8 | Disclosure | REQ-WS-003, REQ-PRIV-001, REQ-PRIV-002, REQ-CTX-007, REQ-SEC-010 | REQ-TEST-015 |
| THR-17 | Sandbox escape through a Wasmtime flaw or a configuration error | B5 | Host access | REQ-ARCH-007, REQ-SBX-004, REQ-CONF-002, REQ-ARCH-008 | REQ-TEST-008, REQ-TEST-013 |
| THR-18 | Audit manipulation or loss | B7 | Loss of evidence | REQ-PERS-003, REQ-OBS-001, REQ-SEC-008 | REQ-TEST-003; integrity deferred to DR-010 |
| THR-19 | Timing side channel from the sandbox | B5 | Inference about the host | REQ-SBX-004 | REQ-TEST-008 |
| THR-20 | Approval spoofing: model-authored text on the approval screen | B1 | The user approves the wrong thing | REQ-APR-002, REQ-APR-008 | REQ-TEST-006 |
| THR-21 | Approval display spoofing with control, invisible or bidirectional characters | B1 | The user approves the wrong thing | REQ-APR-003, REQ-CTX-008 | REQ-TEST-006 |
| THR-22 | Approval swap: action content or grants change after approval | B7 | Unapproved execution | REQ-APR-004, REQ-APR-005, REQ-CAP-007 | REQ-TEST-006 |
| THR-23 | Host tools execute project code (build scripts, macros, tests) with the user's privileges | B5 | Arbitrary code execution | REQ-CONF-001, REQ-CONF-004, REQ-CONF-005, REQ-SBX-010 | REQ-TEST-013 |
| THR-24 | Secret leakage through child environments | B4, B6 | Credential theft | REQ-PRIV-003, REQ-MCP-001, REQ-SBX-004 | REQ-TEST-015 |
| THR-25 | Tool-description poisoning and definition drift from MCP servers | B6 | Prompt injection; silent behaviour change | REQ-MCP-002, REQ-MCP-003, REQ-CTX-005 | REQ-TEST-012 |
| THR-26 | Misdeclared effects: a destructive external tool presented as read-only | B6 | Unapproved destructive call | REQ-MCP-004, REQ-APR-001 | REQ-TEST-012 |
| THR-27 | Snapshot or restore through symbolic links | B8 | Intra-host exfiltration; writes outside the workspace | REQ-WS-002, REQ-WS-005, REQ-TOOL-004 | REQ-TEST-009 |
| THR-28 | Unaudited or automatic promotion into the user's tree | B8 | Silent damage to the project | REQ-WS-007, REQ-SEC-015 | REQ-TEST-009 |
| THR-29 | Inconsistency between the database and the snapshots after a crash | B7 | Corrupt recovery | REQ-WS-004, REQ-PERS-009, REQ-PERS-006 | REQ-TEST-007 |
| THR-30 | A non-local backend used as an exfiltration channel | B2 | Disclosure | REQ-SEC-010 | tests of REQ-SEC-010 |
| THR-31 | Local file tampering or concurrent runtimes on one data directory | B7 | Corruption; disclosure to other local users | REQ-PERS-001, REQ-SEC-008, REQ-WS-001 | tests of REQ-PERS-001 |
| THR-32 | Orphaned processes surviving cancel, deadline or crash | B5 | Resource consumption; stale writes | REQ-CONF-009, REQ-TASK-006 | REQ-TEST-008, REQ-TEST-013 |
| THR-33 | Model-based summarization laundering injected content into runtime text | B2 | Instruction bypass | REQ-CTX-004, REQ-MDL-008 | REQ-TEST-010 |
| THR-34 | Approval fatigue: routine prompts train the user to approve without reading | B1 | Human control erodes | REQ-APR-001, REQ-SEC-014 | review (REQ-SEC-014) |
| THR-35 | The dependency step as an egress channel: a declared host tool with `network = true` reaches any destination, so project content (a crafted manifest, a build script's request) can leave the host without a destination check | B5 | Disclosure | REQ-CONF-007 (declared by the administrator, `external_read`, approval per profile, every other clause in force), REQ-CAP-001 (a `host_tool` grant per task), REQ-CONF-004 (the status and the approval name the step and its network), REQ-SEC-006 (amended: the exception is stated) | REQ-TEST-013 (the offline tool cannot connect; the step runs in its own worker) |

## Residual risks, accepted

| Risk | Why accepted | Owner of the residual |
|---|---|---|
| A compromised operating system or runtime process | mimlys is one layer; it cannot defend against its own host | User, operating system |
| A user who approves without reading | The screen is rendered by the runtime and reserved for real risk; the rest is the user's attention | User |
| A deliberately misdeclared effect by an administrator | Administrators are trusted to the extent of their configuration | Administrator |
| A WASM-hosted interpreter with its own defect | Bounded by the triple guard and WASI; pinned builds | Maintainers, through pinning |
| A workspace size limit that is best-effort between step boundaries | No portable kernel quota; the worker's OS limits add a ceiling where available | Administrator |
| Information in the model's `reason` that the user chooses to believe | Labelled untrusted and separated on the screen | User |

## Review log

| Date | Trigger | Change |
|---|---|---|
| 2026-10-02 | Baseline | Register created from the draft threat catalog (T-01 … T-18 of the draft external design) and the startup review findings R-01 … R-21; THR-20 … THR-34 added |
| 2026-10-27 | M1 (RFCs 001–004, 010) | THR-02, THR-03, THR-09, THR-20 … THR-22 verified by the M1 suites (REQ-TEST-004 … 006); no new threat |
| 2026-10-27 | M2 (RFC 005) | THR-04 … THR-06, THR-11, THR-17, THR-19, THR-32 verified on three platforms (REQ-TEST-008); the worker's death contained (REQ-SEC-011) |
| 2026-10-28 | M3 (RFC 006) | THR-07, THR-08, THR-27 … THR-29 verified (REQ-TEST-007, REQ-TEST-009); timed collection replaced by ownership, which removes a collector from the trust base |
| 2026-10-29 | M4 (RFCs 007, 008) | THR-01, THR-10, THR-16, THR-30, THR-33 verified (REQ-TEST-010, REQ-TEST-015); the backend's locality is a policy attribute recorded in the audit |
| 2026-10-29 | M5 (RFC 009) | THR-24 … THR-26 verified against the test server and rypsok 0.7.0 (REQ-TEST-012); effects come from the administrator, never from annotations |
| 2026-10-31 | M6a (RFC 011) and the design review of 0.6 | THR-23 and THR-32 verified on Linux (REQ-TEST-013; K through `cgroup.kill` where the session delegates a cgroup, else the subreaper walk); THR-35 added for the dependency step; INV-13 and REQ-SEC-006 amended to state the exception |
| 2026-11-03 | M6b (RFC 011 §3, §7) and release 0.7.0 | THR-23 and THR-32 verified on Windows (REQ-TEST-013: the AppContainer, the Job Object's kill-on-close, every mandatory clause enforced on `windows-latest`); THR-35's dependency step shown by its capability; two platform findings recorded in the M6b report §4: `cargo` cannot run inside an AppContainer on current Windows (the `NUL` device), and two installations writing one directory's permissions at once lost one another's entry (serialized by a named mutex since); the permission record names a directory in one spelling, so a runtime entry is recognised under every form; no new threat |
