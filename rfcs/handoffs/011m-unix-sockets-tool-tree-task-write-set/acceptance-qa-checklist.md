# Handoff 011m — Acceptance checklist (0.10)

- [ ] A confined host tool reaches no Unix socket outside its write set, on Landlock ABI 9 or later and on the seccomp rung below it; `status` names the primitive; F and N say `enforced` only then (A-01, X-02)
- [ ] The documented toolchains still work under each rung (measured), or what breaks is named
- [ ] A tool's tree is terminated when the tool exits by itself; a survivor is reported on every platform; an `Err` from `try_wait` kills the tree (X-01, A-37; REQ-CONF-009)
- [ ] The tool layer has no right on the worker's cgroup (A-36)
- [ ] On Linux a tool reaches only its own task's workspace; macOS as measured, or reported as a residual (A-06)
- [ ] Each worker's own clause report is checked before it is used; the worker refuses `run_host_tool` with a short report (A-05)
- [ ] `.cmd` and `.bat` commands are refused at load (A-22)
- [ ] A host tool's `HOME` and `TMPDIR` directory is per execution and removed after it
- [ ] Tests by state with mutations; gate; CI per platform with skips; book; CHANGELOG citing the audit ids; linear history
