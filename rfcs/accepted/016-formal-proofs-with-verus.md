# RFC 016 — Formal proofs with Verus, kept narrow

**Status.** Accepted (2026-11-11, by the owner, on condition of the corrections C1 to C4 of their review, made with C5 to C8 in this text; `.git-exclude/reviewed/owner-review-rfc-016/README.md`). Release 0.10. Normative text: REQ-TEST-018.
**Tracks.** The owner's rule of 2026-11-10: start Verus from the minimum and widen it only on evidence. Spike S3 (handoff 011k, `.git-exclude/review-requests/S3-verus-spike/README.md`), reproduced by the architect.
**Touches.** `crates/mimlys-exec/src/confine/dacl_wait.rs` (the first proof), a new `verify/` directory outside the workspace's members, `crates/mimlys-exec/Cargo.toml` (one lint line), `.github/workflows/ci.yml` (one new job), `docs/src/maintainers/repository-guards.md`; requirements §23 (new REQ-TEST-018); external design §13.5.
**Release.** 0.10.

## Summary

Use Verus to prove a small number of pure functions that a safety promise rests on. The specifications live in the production file as attributes that only the checker reads, so the proof checks the code the product compiles, and the published crates do not depend on Verus. The first and, for now, only proof is the Windows permission-lock decision. Every further proof is a separate decision, made against the rules of this RFC.

## Motivation

- The spike (S3) proved, in 0.4 s, of the pure core `decide_from`, that the permission-lock decision never writes without the lock, refuses when the wait runs out, and never treats a lock the system will not give as free (P1 to P3).
- Of P4 it proved less:
  - every call of `attempt` inside `decide` passes `0` or the saturated limit;
  - `wait_ms` saturates;
  - the one second call in the code sits in the `Held` branch.

  It did **not** prove that no attempt is made outside that branch, the order of the calls, or their number. The tests hold all three: `req_conf_008_free_lock_writes_without_waiting` (exactly one attempt), `…_held_then_released_writes_after_the_wait` (`[0, limit]`) and `…_held_past_the_wait_refuses` (two attempts, no third).
- All three required mutations, and five more, fail the proof. Two survive, both held by those tests: the first attempt waiting (x5), and an attempt added outside the `Held` branch. The architect reproduced the passing proof and one failing mutation.
- The cost was measured: about one day of effort, no Cargo dependency, `Cargo.lock` unchanged, the publish dry run unchanged, a 40-second Linux CI job (mostly installing the toolchain), and about 40 lines of inert text in one file.
- A test shows that some inputs behave; a proof shows that all of them do. That matters here because the proof guards the decision that D1's remedy rests on (D1 itself was a lost update in a write that had no lock; this lock and its decision are the remedy), and little elsewhere.

## Decision

1. **The first proof lands.** The spike's three production edits:
   - `wait_ms`, the saturating wait;
   - `decide_from`, the closure-free core;
   - `if let` in place of the derived `==` that Verus treats as opaque.

   Also: the `cfg_attr(verus_keep_ghost, …)` specifications, `verify/` with its runner, and the `unexpected_cfgs` lint line for `verus_keep_ghost` in `mimlys-exec`. Behaviour and every existing test are unchanged.
2. **The rules for any proof (REQ-TEST-018, new).**
   - **Where.** Only on a pure function: no I/O, no async, no `serde`, no `unsafe`. Its property must be stated as a requirement or a threat control, so that a proof is tied to a promise.
   - **How.** The specifications are in the production file, inside `cfg_attr(verus_keep_ghost, …)` or `#[cfg(verus_keep_ghost)]` only: inert for the product's compiler, never in a public signature.
   - **What is checked.** The checker reads the production file itself, never a copy or a generated variant.
   - **What may not change.** No published crate gains a dependency, and `cargo publish --workspace --dry-run` and `cargo deny` stay unchanged.
   - **What every proof carries.** Mutations that fail it, the mutations that survive and what holds them instead, its run time, and what it does not prove, stated in `repository-guards.md`. The CI job runs at least one recorded mutation and **requires it to fail**, so that a checker that silently proved everything would turn the job red (a vacuity guard).
3. **CI.** A new job, Linux only, so that no existing job changes (the rule of `ci.yml`).
   - Its Verus is a pinned release whose archive's SHA-256 is **compared** with a recorded value, failing on a mismatch (not only printed), and cached by version. Its Rust toolchain is the one that Verus names (1.99.1 today), pinned in the job and moved only with a Verus upgrade, so the job does not change the day stable Rust moves on.
   - The job is **required**: the proof is deterministic, and a red job names the postcondition that broke. The download is retried and cached, as handoff 005b's are.
   - A Verus upgrade is deliberate, moves the toolchain with it, and is done as its own change.
4. **What is not claimed.**
   - P1 to P3 are proved of `decide_from`. `decide` has no postcondition: its one-line call of the core is inspected, not proved.
   - P4's part is proved **under a hypothesis**. `decide`'s `requires` constrain the closure's precondition (it admits exactly `0` and the limit), and no caller of `decide` is verified, so that contract is never checked against the real closure. The encoding is sound and not vacuous, since such a closure exists, but it is not a checked contract.
   - No attempt outside the `Held` branch, the order of the calls and their number are the tests' job (above).
   - The caller in `windows::acl`, which acts on the decision, is outside the proof, and what Windows does with the mutex is the system's.
   - The book and the threat register say "proved" only of P1 to P3, and of the part of P4 that is proved.
5. **The second candidate waits.** The task state machine's transition rules (REQ-TASK-002) are the next candidate. They need a split of `mimlys-core` first: a leaf module of fieldless state and input kinds, and a pure `next_kind`. That split is its own handoff, decided by the owner after this RFC's first proof has run in CI for a release. It must not move the persisted type or change the event vocabulary, and the proof's property must be REQ-TASK-002's transition table itself, line for line, so that the table and the specification are visibly one thing. Nothing else is a candidate until then.
6. **What lands from the spike, and what does not.** The throwaway workflow stays on the spike's branch and goes with it. `verify/README.md` and the comments in `dacl_wait.rs` say what the proof is, not "spike S3". The new job's install follows handoff 005b's rule: pinned, hashed, cached, the download retried.

## Alternatives considered

- **No Verus.** The tests already hold the decision. Rejected for this one function, because the cost is measured and small, and the decision is the one D1's remedy rests on. Kept as the default everywhere else.
- **A generated file instead of in-file annotations** (`verify/` adds the specifications to a copy of the production text). This keeps the source clean, but then the proof checks a derived file, and the risk of drift moves into the generator. Rejected: the point of the proof is the text that ships.
- **A Cargo dependency on Verus's crates.** Impossible while they are not on crates.io, and it would put a verification tool into every user's build. Rejected.
- **An informative (not required) CI job.** A proof that may be red is a proof nobody reads. Rejected, given the job's determinism.

## Consequences

- One file in `mimlys-exec` carries about 40 lines that only the checker reads, and the manifest declares the cfg. A reader sees the specifications beside the code they describe.
- One more pinned tool in CI, with its toolchain. Upgrades are deliberate.
- No change for users: no dependency, no feature, no behaviour.

## Open questions

None for the owner beyond acceptance. The handoff states the job's exact pin and hash.

## References

Spike S3 (handoff 011k and its review request); handoff 011f and investigation D1; requirements REQ-CONF-008, REQ-TEST-013, REQ-TEST-014; threat THR-36; `repository-guards.md`.
