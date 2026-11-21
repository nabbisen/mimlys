#!/bin/sh
# Runs a command with a real, empty, delegated cgroup that passes `cpu` and
# `pids` on (REQ-CONF-013; handoff 013d item 3): the arrangement in which the
# worker can stand in a leaf beside `tool` and `cpu.max` can be set on `tool`.
#
# Usage: scripts/with-delegated-cgroup.sh COMMAND [ARGS...]
#
# It must itself run in a cgroup the user may write, with `cpu` and `pids`
# among its controllers: a unit with delegation, for instance
#
#     sudo systemd-run --wait --pipe --collect -p Delegate=yes -p User="$USER" \
#         scripts/with-delegated-cgroup.sh cargo test ...        (CI)
#     systemd-run --user --scope -p Delegate=yes \
#         scripts/with-delegated-cgroup.sh cargo test ...        (a desktop session)
#
# The cgroup the process lives in holds a process and so cannot pass a
# controller on ("no internal processes"): the script moves itself into a
# sibling `runner`, enables the controllers, and makes an empty `base` beside it.
# `MIMLYS_TEST_CGROUP_BASE` names `base` for the test that asks the kernel
# itself, and `MIMLYS_PROBE_CGROUP_BASE` makes the workers of a test build
# arrange their cgroups in it. `memory` is left out on purpose: the probe tool's
# `alloc` allocates without touching, which a memory limit of the cgroup
# (unlike `RLIMIT_AS`) does not see, and no suite has run on that rung before.
set -eu
if [ "$#" -eq 0 ]; then
    echo "usage: $0 COMMAND [ARGS...]" >&2
    exit 2
fi
cg="/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)"
mkdir "$cg/runner"
echo $$ > "$cg/runner/cgroup.procs"
echo "+cpu +pids" > "$cg/cgroup.subtree_control"
mkdir "$cg/base"
MIMLYS_TEST_CGROUP_BASE="$cg/base"
MIMLYS_PROBE_CGROUP_BASE="$cg/base"
export MIMLYS_TEST_CGROUP_BASE MIMLYS_PROBE_CGROUP_BASE
echo "delegated cgroup: $cg/base (controllers: $(cat "$cg/base/cgroup.controllers"))"
status=0
"$@" || status=$?
# The workers' cgroups are gone; what a crash left is empty and goes too.
rmdir "$cg/base"/mimlys-* 2>/dev/null || true
rmdir "$cg/base" 2>/dev/null || true
exit "$status"
