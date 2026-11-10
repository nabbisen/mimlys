# Handoff 011i — The sweeper's stream test needs a margin, not a race (0.9)

**RFC.** [RFC 011](../../done/011-confinement-backends.md) (Amendment 2, implementation decisions, point 6: the sweeper) · **Specification.** REQ-CONF-009, REQ-TEST-013 · **Found.** `main`'s CI on `11d3748` (run `42801724567`): only the job `macOS x86_64 under Rosetta` failed, in `crates/mimlys/tests/sweeper.rs:151`, `req_conf_009_sweeper_has_its_own_group_and_holds_no_stream`: "end of file on the owner's output came with the owner, not with the sweeper (2.004350042s)".

## Status (2026-11-10): merged

On `main` (`41d4e03`; CI `42806070020`, 9 of 9; 20 of 20 runs under Rosetta). The three sweeper tests decide by process state, and the durations they hold are far longer than anything they wait for. The same pass over the other test files is a 0.10 hygiene item (ROADMAP). The throwaway branch `011i-throwaway-rosetta-20` is the owner's to delete. Nothing is open here.

## 1. Cause (the architect's reading; confirm it)

The test tells the two cases apart by time: the sweeper sleeps 3 s, and end of file must come within 2 s of the group kill. Under Rosetta, a cold, translated reader thread can take two seconds on its own. It measured 2.004 s, still a second before the sweeper's end, so the property held and the margin did not. Nothing in the product changed between this run and the last green one.

## 2. Correction

Give the test a margin that a slow runner cannot cross. Let the sweeper hold for a long, explicit time (for example 20 s), and require end of file well before it (for example within 10 s), with a comment saying why the two numbers are far apart. Better still, if a non-timing signal exists, use it: for example, the sweeper is still alive (`ps`) at the moment end of file arrives, which proves end of file did not wait for it. Check the file's other timing tests the same way, and say which you changed. No product change.

## 3. Evidence

The `sweeper` test binary run 20 times on the Rosetta job (a throwaway workflow), 0 failures. Mutation: the sweeper given the owner's stdout; the corrected test fails.

## 4. Return

`.git-exclude/review-requests/011i-sweeper-test/README.md`: the change, the 20-run count, the mutation, CI 9 of 9 with skips.
