#!/bin/sh
# The Linux full tier is asserted (REQ-PLAT-003, handoff 002d item 11).
#
# Usage: scripts/check-skip-tier.sh [SKIPS_LOG]      (default: target/tmp/skips.log)
#
# `common::skip` (crates/mimlys/tests/common) and the unit-test helper of
# mimlys-exec append one line per skipped test to `skips.log`:
#
#     SKIPPED <test>: <reason>
#
# A Linux CI job must be at the `full` tier (external design §13), so a skip
# whose *reason* names the tier ("this machine is tier core ...", "tier basic")
# means the runner is not at the tier the job exists to prove: the script
# prints the line and exits 1. Only the reason is read: a test may have "tier"
# in its name (m6a_host_tool_approval_shows_the_tier) and skip for another cause.
#
# Allowed by name (they are not skips of the tier):
#   - m6a_demonstration_cargo_test_offline, when the reason begins "no rustup toolchain"
#     (the runner has no rustup toolchain to hand to the demonstration); its
#     other skip, on the tier, is not allowed;
#   - m5_real_rypsok (MIMLYS_RYPSOK_BIN) and m4_real_engine_repairs_the_defect
#     (MIMLYS_M4_BACKEND_URL, MIMLYS_M4_MODEL): opt-in tests that skip on every
#     CI machine on purpose. They print their skip and do not write the file today;
#     the names are here so that counting them later cannot turn a job red.
#
# A line that is not "SKIPPED <test>: <reason>" fails too: a file the rule cannot
# read is never taken for a pass. A missing or empty file is 0 skips: exit 0.
#
# Exit status: 0 no tier skip; 1 a tier skip or an unreadable line; 2 usage.
set -eu

if [ "$#" -gt 1 ]; then
    echo "usage: $0 [SKIPS_LOG]" >&2
    exit 2
fi
log="${1:-target/tmp/skips.log}"

if [ ! -s "$log" ]; then
    echo "tier skips on this machine: 0 (no skips.log or empty: $log)"
    exit 0
fi

# awk prints each offending line, prefixed by the cause, and the count at the end.
bad=$(awk '
    /^[[:space:]]*$/ { next }
    {
        line = $0
        if (substr(line, 1, 8) != "SKIPPED ") { print "unreadable: " line; n++; next }
        rest = substr(line, 9)
        i = index(rest, ": ")
        if (i == 0) { print "unreadable: " line; n++; next }
        test = substr(rest, 1, i - 1)
        reason = substr(rest, i + 2)
        if (test == "m5_real_rypsok" || test == "m4_real_engine_repairs_the_defect") next
        if (test == "m6a_demonstration_cargo_test_offline" && index(reason, "no rustup toolchain") == 1) next
        if (reason ~ /(^|[^A-Za-z0-9_])[Tt]ier([^A-Za-z0-9_]|$)/) { print "tier: " line; n++ }
    }
' "$log")

if [ -n "$bad" ]; then
    echo "FAIL: the Linux full tier is asserted (REQ-PLAT-003), and these skips are the tier's or unreadable:" >&2
    printf '%s\n' "$bad" >&2
    exit 1
fi
echo "tier skips on this machine: 0 ($log)"
