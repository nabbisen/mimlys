# Handoff 002b — Test hygiene (release 0.10)

**RFC.** [RFC 002](../../done/002-decision-schema-and-validation-order.md) (the validator, which the hostile corpus targets first) · **Specification.** REQ-TEST-001 (every `T` requirement named by a test), REQ-SEC-013 (the hostile corpus), REQ-TEST-013 · **From.** The replanning report §3 C; handoff 011i's rule.

1. **The untested requirements.** `scripts/check-req-tests.sh` lists the `T` requirements that no test names (31 of 189 at 0.9.0, before RFC 013 to 015 added theirs). For each one: if an existing test verifies it, name it in that test's doc comment and say in the request what the test asserts; if none does, write one; if it is met structurally, add it to `scripts/req-test-map.txt` with the reason. Never name a requirement on a test that does not verify it. Work in slices of about ten per request. RFC 013 to 015's new requirements are named by their own handoffs.
2. **A data-driven hostile corpus.** Today REQ-SEC-013's corpus is a named set of tests (`repository-guards.md` §3a). Make it data:
   - one directory of cases (hostile decisions, paths, tool results, MCP descriptions, modules), each a small file with its expected outcome (the rejection code, the denial reason, the redaction);
   - one harness that runs them all;
   - the existing hostile tests' inputs moved into it, with nothing lost (list them).

   A case added later is a file, not code.
3. **Timing bounds.** Apply handoff 011i's rule to the other test files: a test decides by process or store state, and any duration it holds is far longer than anything it waits for. List every short bound found, and what replaced it.

Return: one request per item, `.git-exclude/review-requests/002b-<item>/README.md`.
