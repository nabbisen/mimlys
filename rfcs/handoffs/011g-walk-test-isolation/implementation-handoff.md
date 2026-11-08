# Handoff 011g — The descendant-walk test must not walk the test process (release 0.8)

**RFC.** [RFC 011](../../accepted/011-confinement-backends.md) (clause K, Linux's walk rung) · **Specification.** REQ-CONF-009, REQ-TEST-013 · **Found.** `main`'s CI after 011c-6: run `42461069396` (`e25f75f`) and `42461134298` (`2e1384e`), job `check (ubuntu-24.04-arm)`, both failed in `confine::linux::tests::rfc_011_descendant_walk_kills_the_tree` (`assertion failed: descendants_of(me).is_empty()`, `tests.rs:19`); the other eight jobs passed. The PR's own CI of 011c-6 passed.

## 1. Cause (the architect's reading; confirm it)

The test walks and kills the descendants of **the test process itself** (`descendants_of(std::process::id())`, `kill_descendants(me, …)`) and assumes it has no other children. Since 011c-6, other unit tests of the same binary (`confine/macos/xcrun/tests.rs`, which runs on every Unix with a stand-in `sh` script, one of them hanging until killed) start children of the test process in parallel threads. The walk then sees them, may kill another test's child, and finds a new one after its kill. A product defect is not involved: in the product the root is the worker, whose only children are its tools.

## 2. Correction

Root the walk at a process of the test's own: start a helper (`sh -c 'sleep 30 & sleep 30 & wait'` or the probe tool), walk `descendants_of(helper)` and `kill_descendants(helper, …)`, and assert on that subtree only (the grandchildren found, none left after the kill), then reap the helper. Search every test that walks or kills from `std::process::id()` (Linux and macOS: the membership tests are inside a sandboxed worker and are not affected; say so if you find otherwise) and fix them the same way. No change to the product.

## 3. Tests and evidence

The corrected test names REQ-CONF-009. Mutation: `kill_descendants` returning without killing; the test fails. Evidence that the race is gone: the `mimlys-exec` unit tests run 20 times in a row on `ubuntu-24.04-arm` (one throwaway workflow job), 0 failures, in the request.

## 4. Return

`.git-exclude/review-requests/011g-walk-test/README.md`: the change, the gate, CI per platform with skips, the mutation, the 20-run count.
