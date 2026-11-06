#!/usr/bin/env python3
"""SPIKE S1, Q7 and Q8: the iterative method of handoff 011d, on macOS.

Start from a small profile (the declared ~/.cargo/bin and ~/.rustup, the worker's
directory, /usr), run `cargo test --offline` of the demonstration project
under it, read the sandbox denials from the unified log, add the single
file rule each denial names, repeat. Mach services and other operations are
recorded and NOT added in phase 1; if the run still fails when the file
rules have converged, phase 2 adds the mach services seen so that the file
set can be finished, and the report says which phase each fact came from.
Prints lines starting with "S1 "."""
import collections, datetime, os, re, subprocess, sys, time

S = os.environ["S1_DIR"]
HOME = os.path.expanduser("~")
PROBE = f"{S}/bin/probe"
WS = f"{S}/ws"
PROJ = f"{WS}/proj"
DENY = re.compile(r"Sandbox: (\S+)\((\d+)\) deny\(\d+\) (\S+)\s*(.*)$")


def say(msg):
    print("S1 " + msg, flush=True)


def sh(cmd, **kw):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True, **kw)


# the demonstration project (a path dependency, a passing test)
os.makedirs(f"{PROJ}/src", exist_ok=True)
os.makedirs(f"{PROJ}/arith/src", exist_ok=True)
open(f"{PROJ}/Cargo.toml", "w").write('[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\narith = { path = "arith" }\n\n[workspace]\nmembers = ["arith"]\n')
open(f"{PROJ}/arith/Cargo.toml", "w").write('[package]\nname = "arith"\nversion = "0.1.0"\nedition = "2021"\n')
open(f"{PROJ}/arith/src/lib.rs", "w").write("pub fn add(a: i32, b: i32) -> i32 { a + b }\n")
open(f"{PROJ}/src/lib.rs", "w").write("pub fn total(xs: &[i32]) -> i32 { xs.iter().fold(0, |a, x| arith::add(a, *x)) }\n#[cfg(test)] mod t { #[test] fn sums() { assert_eq!(super::total(&[1,2,3]), 6); } }\n")

files = {}  # (op-class, path) -> first process name
mach, other = collections.OrderedDict(), collections.OrderedDict()
seed_reads = [f"{HOME}/.cargo/bin", f"{HOME}/.rustup", "/usr", f"{S}/bin", S]


def profile(extra_files, extra_mach):
    lines = [
        "(version 1)", "(deny default)", "(allow process-fork)", "(allow process-exec*)",
        "(allow file-read-metadata)", "(allow sysctl-read)", "(allow signal (target self))",
        '(allow file-read* (literal "/dev/urandom") (literal "/dev/random") (literal "/dev/null") (literal "/dev/zero"))',
        f'(allow file-write* (subpath "{WS}") (literal "/dev/null"))',
        f'(allow file-read* (subpath "{WS}"))',
    ]
    for p in seed_reads:
        lines.append(f'(allow file-read* (subpath "{p}"))')
    for op, path in extra_files:
        lines.append(f'(allow {op} (literal "{path}"))')
    for name in extra_mach:
        lines.append(f'(allow mach-lookup (global-name "{name}"))')
    return "\n".join(lines) + "\n"


def run_once(prof_text, label):
    path = f"{S}/prof/iter.sb"
    open(path, "w").write(prof_text)
    start = (datetime.datetime.now() - datetime.timedelta(seconds=1)).strftime("%Y-%m-%d %H:%M:%S")
    env = (f"/usr/bin/env -i HOME={WS}/home PATH={HOME}/.cargo/bin:/usr/bin:/bin RUSTUP_HOME={HOME}/.rustup "
           f"RUSTUP_TOOLCHAIN=stable CARGO_TARGET_DIR={WS}/target TMPDIR={WS}/tmp")
    os.makedirs(f"{WS}/home", exist_ok=True)
    cmd = f"{PROBE} apply {path} -- {env} /bin/sh -c 'cd {PROJ} && {HOME}/.cargo/bin/cargo test --offline'"
    t0 = time.time()
    r = subprocess.run(["perl", "-e", "alarm shift; exec @ARGV", "240", "/bin/sh", "-c", cmd], capture_output=True, text=True)
    dt = time.time() - t0
    time.sleep(2)  # let the log catch up
    log = sh(f"log show --start '{start}' --style compact --info --predicate 'process == \"kernel\" AND eventMessage CONTAINS \"Sandbox:\"'")
    text = log.stdout
    if "Sandbox:" not in text:
        log2 = sh(f"sudo -n log show --start '{start}' --style compact --info --predicate 'process == \"kernel\" AND eventMessage CONTAINS \"Sandbox:\"'")
        if "Sandbox:" in log2.stdout:
            text = log2.stdout
    return r, dt, text


def parse(text):
    out = []
    for line in text.splitlines():
        m = DENY.search(line)
        if m:
            out.append((m.group(1), m.group(3), m.group(4).strip()))
    return out


added, added_mach = [], []
phase = 1
history = []
for rnd in range(1, 41):
    r, dt, text = run_once(profile(added, added_mach), f"round {rnd}")
    dens = parse(text)
    ok = r.returncode == 0
    say(f"iter round {rnd} (phase {phase}): exit={r.returncode} {dt:.0f}s, {len(dens)} denial lines, files so far {len(added)}, mach so far {len(added_mach)}")
    if rnd == 1:
        say("iter first output tail: " + (r.stdout + r.stderr)[-600:].replace("\n", " | "))
        if not dens:
            say("iter NOTE: no sandbox denials were read from the log (log show unavailable or format changed); raw sample follows")
            say("iter log sample: " + text[:500].replace("\n", " | "))
    if ok:
        say(f"iter RESULT: cargo test --offline succeeded in round {rnd} (phase {phase})")
        break
    new = 0
    for proc, op, arg in dens:
        if op.startswith("file-") and arg.startswith("/"):
            if op.startswith("file-write"):
                other[(op, arg)] = proc
                continue
            key = (op, arg)
            if key not in files:
                files[key] = proc
                added.append(key)
                new += 1
        elif op == "mach-lookup":
            if arg not in mach:
                mach[arg] = proc
                if phase == 2:
                    added_mach.append(arg)
                    new += 1
        else:
            other[(op, arg)] = proc
    if new == 0:
        if phase == 1 and mach:
            phase = 2
            say("iter: file rules converged; phase 2 adds the mach services seen")
            added_mach.extend(mach.keys())
            continue
        say("iter: no new rule to add; stopping")
        say("iter last output: " + (r.stdout + r.stderr)[-1200:].replace("\n", " | "))
        break

say("iter ===== additions to the seed (file reads; literal rules, grouped by directory) =====")
seed_note = ", ".join(seed_reads)
say(f"iter seed: {seed_note}")
by_dir = collections.defaultdict(list)
for (op, path), proc in files.items():
    by_dir[os.path.dirname(path)].append((op, os.path.basename(path), proc))
for d in sorted(by_dir):
    ops = sorted({o for o, _, _ in by_dir[d]})
    procs = sorted({p for _, _, p in by_dir[d]})
    say(f"iter file {d}/  ({len(by_dir[d])} entries; ops {','.join(ops)}; first denied for {','.join(procs)}; e.g. {by_dir[d][0][1]})")
say("iter ===== mach services denied (NOT added in phase 1) =====")
for name, proc in mach.items():
    say(f"iter mach {name}  (denied for {proc})")
say("iter ===== other denied operations =====")
for (op, arg), proc in list(other.items())[:80]:
    say(f"iter other {op} {arg}  ({proc})")
