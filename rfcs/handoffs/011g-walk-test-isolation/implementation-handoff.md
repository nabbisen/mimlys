# Handoff 011g — The descendant-walk test must not walk the test process (release 0.8)

**RFC.** [RFC 011](../../accepted/011-confinement-backends.md) (clause K, Linux's walk rung) · **Specification.** REQ-CONF-009, REQ-TEST-013 · **Found.** `main`'s CI after 011c-6: run `42461069396` (`e25f75f`) and `42461134298` (`2e1384e`), job `check (ubuntu-24.04-arm)`, both failed in `confine::linux::tests::rfc_011_descendant_walk_kills_the_tree` (`assertion failed: descendants_of(me).is_empty()`, `tests.rs:19`); the other eight jobs passed. The PR's own CI of 011c-6 passed.

## Status (2026-11-08, third)

Attempt 6 of run `42465321601`: seven of nine green, both Linux jobs included with the walk test `ok`; the other two failed only on 504s (mdbook, the interpreter). No further re-run: the architect puts PR #24 on `main` after 005b, whose CI no longer depends on those downloads, and `main`'s own CI checks it. Nothing more to do here.

## Status (2026-11-08, second): follow-up approved, merge waits for a green run

The grandchild follow-up (PR #24, `6436b28`) is approved: three descendants with the grandchild found only past one level, both mutations killed. Its CI (run `42465321601`) failed only on HTTP 504s from GitHub's downloads; the walk test passed on every Linux job that reached it. A test-only change is still merged on a green run: **re-run the failed jobs once the downloads answer** (the architect checked at the time of review: still 504) and update the run id at the top of the request. Handoff 005b removes the CI's dependence on those downloads.

## Status (2026-11-08): merged; one small follow-up

011g is on `main` (`d3d913a`, CI `42462179051`, 9 of 9; 20 of 20 runs clean against 10 of 20 failing on unfixed `main`). One thing the rewrite lost: the old test reached a **grandchild** (test process → `sh` → `sleep`), the new one only children of the helper (helper → two `sleep`s), so the walk's following of ancestry beyond one level is no longer tested. On a short branch: give the helper a grandchild (for example `sh -c 'sh -c "sleep 30; :" & sleep 30 & wait'`, where the trailing `:` keeps the inner shell from replacing itself with `sleep`), assert the walk finds three descendants with the grandchild among them, and that none is left after the kill. Mutation: `descendants_of` returning only direct children; the test fails. Update the request at the top.

## 1. Cause (the architect's reading; confirm it)

The test walks and kills the descendants of **the test process itself** (`descendants_of(std::process::id())`, `kill_descendants(me, …)`) and assumes it has no other children. Since 011c-6, other unit tests of the same binary (`confine/macos/xcrun/tests.rs`, which runs on every Unix with a stand-in `sh` script, one of them hanging until killed) start children of the test process in parallel threads. The walk then sees them, may kill another test's child, and finds a new one after its kill. A product defect is not involved: in the product the root is the worker, whose only children are its tools.

## 2. Correction

Root the walk at a process of the test's own: start a helper (`sh -c 'sleep 30 & sleep 30 & wait'` or the probe tool), walk `descendants_of(helper)` and `kill_descendants(helper, …)`, and assert on that subtree only (the grandchildren found, none left after the kill), then reap the helper. Search every test that walks or kills from `std::process::id()` (Linux and macOS: the membership tests are inside a sandboxed worker and are not affected; say so if you find otherwise) and fix them the same way. No change to the product.

## 3. Tests and evidence

The corrected test names REQ-CONF-009. Mutation: `kill_descendants` returning without killing; the test fails. Evidence that the race is gone: the `mimlys-exec` unit tests run 20 times in a row on `ubuntu-24.04-arm` (one throwaway workflow job), 0 failures, in the request.

## 4. Return

`.git-exclude/review-requests/011g-walk-test/README.md`: the change, the gate, CI per platform with skips, the mutation, the 20-run count.
