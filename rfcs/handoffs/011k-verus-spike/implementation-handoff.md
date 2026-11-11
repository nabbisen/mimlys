# Handoff 011k — Spike S3: one Verus proof of the permission-lock decision (report only)

**RFC.** [RFC 011](../../done/011-confinement-backends.md) §7, the permission lock of handoff 011f · **Specification.** REQ-CONF-008, external design §13.5, limits LIM-O7; threat THR-36 · **Kind.** Spike: a measured answer, not a feature. The owner's rule (2026-11-10): start Verus from the minimum, and widen it only on evidence. This spike is the minimum. Whether Verus is used anywhere else, and under which rules, is decided afterwards, in a short RFC written from your report.

**Status (2026-11-11): done.** The report is accepted and its result reproduced by the architect: the proof passes (3 verified, 0 errors) and mutation m1 fails it. P1 to P3 are proved and P4 in part. Build, publish and `cargo deny` are unchanged. RFC 016, accepted on 2026-11-11, decides adoption from it. **Correction to the record (the owner's C1):** the report's sentence "the in-body assertion proves that the second attempt happens only after a `Held`" overstates. The assertion proves that the one second call *in the code* sits in the `Held` branch; an attempt added outside that branch, the order and the number of attempts are held by the tests. RFC 016 decision 4 says exactly what is proved, and nothing is open in this handoff, and branch `S3-verus-spike` and PR #40 stay until RFC 016 is decided.

## 1. The function and the properties

`crates/mimlys-exec/src/confine/dacl_wait.rs`, `decide`: two attempts at the named mutex (the first without waiting; if held, a second within LIM-O7) become a `Decision`. Prove, for every input the function can receive:

| # | Property | Why it matters |
|---|---|---|
| P1 | The result is `Write` only if an attempt returned `Acquired`, and `WriteAfterAbandoned` only if one returned `Abandoned` | no write without holding the lock (THR-36; D1's lost entries) |
| P2 | If the first attempt is `Held` and the second is `Held`, the result is `Refuse(Held)` | the wait ends in a refusal, never a write on a stale base |
| P3 | Any `Failed` attempt that decides the result gives `Refuse(LockUnavailable)` | a lock the system will not give is never treated as free (011f's correction) |
| P4 | A second attempt happens only when the first was `Held`, and its wait is the limit passed (saturating at `u32::MAX`), never less | the wait is LIM-O7, not an accident |

P1 to P3 are about the outcome; P4 is about the attempts made. If Verus's support for closures (`FnMut`, `FnOnce`) makes P4 unprovable as the function is written, say so. In that case you may extract a closure-free core in the production module, such as `decide_from(first: Attempt, second: Option<Attempt>) -> Decision`, with `decide` reduced to calling the attempts and the core. Prove P1 to P3 on the core, and keep P4 to the existing tests. The extraction must leave behaviour and every existing test unchanged.

## 2. Binding constraints

- **Publishing and the build do not change.** mimlys is on crates.io, and Verus's support crates are not. Nothing in the published crates may depend on Verus, its macros or `vstd`. `cargo build`, `cargo test`, `cargo publish --workspace --dry-run` and `cargo deny` must give the same results as before.
- **The proof checks the production source, not a copy.** Put the verification in a directory of its own (proposal: `verify/`, outside the workspace's members and never published). It should include or reference the very file, or the extracted core, that the product compiles. If Verus cannot do that and only a copy works, stop and report: a copy that can drift is not what we want proven, and I will decide.
- **The installed Verus** (`~/.local/bin/verus`, 0.2026.11.02.8bf68ee); record its exact version. No other install, and nothing outside the project except that binary; scratch goes in `.git-exclude/tmp/`.
- **Time box:** one working day of effort and at most three CI runs on a throwaway branch. Stop at the box, whatever state the proof is in, and report.

## 3. What to measure and report

1. **Integration.** How the proof reaches the production source, the commands, and whether the build, test and publish results are unchanged (each before and after).
2. **The proof.** P1 to P4, each proved, partly proved (say what is left) or not provable (say why): the spec, the proof text and the lines it took.
3. **Mutations.** Each of these must make the proof fail, with Verus's output shown:
   - the second `Held` mapped to `Write`;
   - `Failed` mapped to `Write`;
   - the second attempt's wait set to `limit_ms / 2`.
4. **Cost.**
   - Effort: hours, and what was hard.
   - Verus's run time: cold and warm.
   - What a CI job would need: the installation, the time per run, and pinning the version as for mdbook and cargo-deny.
   - What a future change of `decide` would cost to re-prove.
5. **Your recommendation**, for or against, on two things: a Verus check in CI for this function, and a second candidate (the task state machine's transition rules, REQ-TASK-002). For the second, say what would have to be split out first.

## 4. Return

`.git-exclude/review-requests/S3-verus-spike/README.md`, with Verus's outputs and the proof files in `evidence/`. Nothing is merged. If the extraction of §1 was needed, it goes on the throwaway branch only; I decide whether it lands.
