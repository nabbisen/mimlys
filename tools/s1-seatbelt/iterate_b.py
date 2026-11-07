#!/usr/bin/env python3
"""SPIKE S1b, B3 and B5 (branch s1b-macos-seatbelt only, never merged).

B3: the iterative method of handoff 011d under the Amendment-2 profile shape: start from the
declared toolchain and the developer directory (read from /private/var/db/xcode_select_link, no
command run), run `cargo test --offline` of the demonstration project inside the profile, read the
sandbox denials from the unified log, add what they name, repeat until `Finished` and the test
passes. Then NECESSITY: remove each mach service and each home-directory file (and each other file
group) one at a time and record whether the run still passes; then a greedy pass that yields a
1-minimal set. B5: from the minimal profile, `cargo fetch` of a project with one registry
dependency; the smallest additions; the sibling without them.
Every result line starts with "S1b ". The log is the finding: this script never raises."""
import collections, datetime, glob, json, os, re, shutil, subprocess, sys, time, traceback

S = os.environ["S1_DIR"]
HERE = os.environ["HERE"]
WHAT = os.environ.get("S1B_WHAT", "b3")  # b3 | b5 | b3,b5
HOME = os.path.expanduser("~")
PROBE = f"{S}/bin/probe"
PROBEB = f"{S}/bin/probeb"
WS = f"{S}/ws"
PROJ = f"{WS}/proj"
PROJF = f"{WS}/projfetch"
T0 = time.time()
DEADLINE = T0 + float(os.environ.get("S1B_BUDGET", "4500"))  # seconds for the whole script
DENY = re.compile(r"Sandbox: (\S+)\((\d+)\) deny\(\d+\) (\S+)\s*(.*)$")
OURS = re.compile(r"^(env|sh|bash|zsh|probe|cargo|rustc|rustup|cc|c\+\+|clang|ld|xcrun|xcode-select|ar|dsymutil|strip|lipo|libtool|git|arith|demo.*|fetchdemo.*|build.*|rust-.*|ld64.*|ld-prime|.*-[0-9a-f]{8,})$")


def say(msg):
    print("S1b " + msg, flush=True)


def sh(cmd, **kw):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True, **kw)


def left():
    return DEADLINE - time.time()


def phase_until(seconds):
    """Each phase has its own end, counted from the start of the script."""
    global DEADLINE
    DEADLINE = T0 + seconds


# ---- the demonstration projects ----------------------------------------------------
def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w").write(text)


write(f"{PROJ}/Cargo.toml", '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\narith = { path = "arith" }\n\n[workspace]\nmembers = ["arith"]\n')
write(f"{PROJ}/arith/Cargo.toml", '[package]\nname = "arith"\nversion = "0.1.0"\nedition = "2021"\n')
write(f"{PROJ}/arith/src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n")
write(f"{PROJ}/src/lib.rs", "pub fn total(xs: &[i32]) -> i32 { xs.iter().fold(0, |a, x| arith::add(a, *x)) }\n#[cfg(test)] mod t { #[test] fn sums() { assert_eq!(super::total(&[1,2,3]), 6); } }\n")
write(f"{PROJF}/Cargo.toml", '[package]\nname = "fetchdemo"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\nitoa = "=1.0.10"\n\n[workspace]\n')
write(f"{PROJF}/src/lib.rs", "pub fn f(n: u32) -> String { itoa::Buffer::new().format(n).to_string() }\n")
os.makedirs(f"{WS}/home", exist_ok=True)
os.makedirs(f"{WS}/tmp", exist_ok=True)

# ---- where things are ----------------------------------------------------------------
CARGO_SHIM = sh("which cargo").stdout.strip()
RUSTUP_HOME = os.path.realpath(f"{HOME}/.rustup")
SYSROOT = sh(f"RUSTUP_HOME={RUSTUP_HOME} rustc --print sysroot").stdout.strip()
LINK = "/private/var/db/xcode_select_link"
DEV = None
try:
    DEV = os.path.realpath(os.readlink(LINK))  # read the link: no xcode-select, no xcrun
except OSError as e:
    say(f"dev: cannot read {LINK}: {e}")
if not DEV or not os.path.isdir(DEV):
    cand = "/Library/Developer/CommandLineTools"
    DEV = cand if os.path.isdir(cand) else None
    say(f"dev: falling back to {DEV}")
VARIANTS = {
    "shim": dict(cargo=CARGO_SHIM, bindir=os.path.dirname(CARGO_SHIM), bindir_real=os.path.dirname(os.path.realpath(CARGO_SHIM)), rustup=True),
}
if SYSROOT and os.path.isfile(f"{SYSROOT}/bin/cargo"):
    VARIANTS["direct"] = dict(cargo=f"{SYSROOT}/bin/cargo", bindir=f"{SYSROOT}/bin", bindir_real=os.path.realpath(f"{SYSROOT}/bin"), rustup=False)


def diag():
    say("diag: HOME=%s cargo=%s (-> %s) sysroot=%s" % (HOME, CARGO_SHIM, os.path.realpath(CARGO_SHIM), SYSROOT))
    say("diag: %s -> %s ; resolved developer directory %s" % (LINK, os.readlink(LINK) if os.path.islink(LINK) else "(not a link)", DEV))
    say("diag (control, outside any profile, for comparison only): xcode-select -p -> " + sh("xcode-select -p 2>&1").stdout.strip())
    say("diag: " + sh("ls /Applications | grep -i xcode | tr '\\n' ' '").stdout)
    say("diag: sw_vers " + sh("sw_vers | tr '\\n' ' '").stdout)


# ---- profile -------------------------------------------------------------------------
SHAPE = [  # the Amendment 2 shape: members only, the process-arguments sysctl refused
    "(allow signal (target same-sandbox))",
    "(deny process-info*)",
    "(allow process-info* (target same-sandbox))",
    '(deny sysctl-read (sysctl-name-prefix "kern.procargs"))',
]


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"')


class Item:
    def __init__(self, kind, rule, op="", arg="", who="seed", label=None):
        self.kind, self.rule, self.op, self.arg, self.who = kind, rule, op, arg, who
        self.label = label or (f"{op} {arg}" if op else rule)

    def to_json(self):
        return dict(kind=self.kind, rule=self.rule, op=self.op, arg=self.arg, who=self.who, label=self.label)


def header(shape=True):
    lines = [
        "(version 1)", "(deny default)", "(allow process-fork)", "(allow process-exec*)",
        "(allow file-read-metadata)", "(allow sysctl-read)", "(allow signal (target self))",
        '(allow file-read* (literal "/dev/urandom") (literal "/dev/random") (literal "/dev/null") (literal "/dev/zero"))',
        f'(allow file-write* (subpath "{WS}") (literal "/dev/null"))',
    ]
    if shape:
        lines += SHAPE
    return lines


def profile_text(items, shape=True):
    return "\n".join(header(shape) + [i.rule for i in items]) + "\n"


def seeds(variant, with_dev=True):
    v = VARIANTS[variant]
    paths = [v["bindir"], v["bindir_real"], RUSTUP_HOME, "/usr", f"{S}/bin", S]
    if with_dev and DEV:
        paths.append(DEV)
    out, seen = [], set()
    for p in paths:
        if p in seen:
            continue
        seen.add(p)
        out.append(Item("seed", f'(allow file-read* (subpath "{esc(p)}"))', "seed", p, "seed", f"seed subpath {p}"))
    return out


def rule_candidates(op, arg):
    """Rules (alternatives, first valid wins) that would allow a denied operation; [] when unknown."""
    if op.startswith("file-") and arg.startswith("/"):
        return "file", [f'(allow {op} (literal "{esc(arg)}"))']
    if op == "mach-lookup":
        return "mach", [f'(allow mach-lookup (global-name "{esc(arg)}"))']
    if op.startswith("ipc-posix-shm"):
        return "other", [f'(allow {op} (ipc-posix-name "{esc(arg)}"))']
    if op == "network-outbound":
        if arg.startswith("/"):
            return "net", [f'(allow network-outbound (remote unix-socket (literal "{esc(arg)}")))']
        m = re.search(r":(\d+)$", arg)
        if m:
            return "net", [f'(allow network-outbound (remote ip "*:{m.group(1)}"))', f'(allow network-outbound (remote tcp "*:{m.group(1)}"))']
    if op in ("network-bind", "network-inbound") and arg.startswith("/"):
        return "net", [f'(allow {op} (local unix-socket (literal "{esc(arg)}")))']
    if op == "user-preference-read":
        return "other", [f'(allow user-preference-read (preference-domain "{esc(arg)}"))']
    return None, []


def parses(rule):
    p = f"{S}/prof/validate.sb"
    open(p, "w").write("(version 1)\n(deny default)\n" + rule + "\n")
    r = subprocess.run(["perl", "-e", "alarm shift; exec @ARGV", "20", PROBE, "apply", p, "--", "/usr/bin/true"], capture_output=True, text=True)
    return "sandbox_init rc=0" in (r.stdout + r.stderr)


# ---- running -------------------------------------------------------------------------
def cmd_for(kind, variant):
    v = VARIANTS[variant]
    path = f"{v['bindir']}:/usr/bin:/bin"
    envs = f"HOME={WS}/home PATH={path} RUSTUP_HOME={RUSTUP_HOME} RUSTUP_TOOLCHAIN=stable TMPDIR={WS}/tmp"
    if kind == "test":
        return PROJ, f"/usr/bin/env -i {envs} CARGO_TARGET_DIR={WS}/target /bin/sh -c 'cd {PROJ} && {v['cargo']} test --offline'"
    return PROJF, f"/usr/bin/env -i {envs} CARGO_HOME={WS}/cargo-home CARGO_TARGET_DIR={WS}/target-fetch CARGO_NET_RETRY=0 CARGO_HTTP_TIMEOUT=30 CARGO_TERM_COLOR=never /bin/sh -c 'cd {PROJF} && {v['cargo']} fetch'"


def passed(rc, out, kind):
    if kind == "test":
        return rc == 0 and re.search(r"test result: ok\. 1 passed", out) is not None and "Finished" in out
    return rc == 0 and bool(glob.glob(f"{WS}/cargo-home/registry/cache/*/itoa-1.0.10.crate"))


def run_profile(text, kind, variant, timeout=240):
    """Runs the command inside the profile; returns (rc, output, seconds, start, pid_lo, pid_hi)."""
    path = f"{S}/prof/it.sb"
    open(path, "w").write(text)
    if kind == "test":
        shutil.rmtree(f"{WS}/target", ignore_errors=True)
    else:
        shutil.rmtree(f"{WS}/cargo-home", ignore_errors=True)
        shutil.rmtree(f"{WS}/target-fetch", ignore_errors=True)
        try:
            os.remove(f"{PROJF}/Cargo.lock")
        except OSError:
            pass
    cwd, cmd = cmd_for(kind, variant)
    start = (datetime.datetime.now() - datetime.timedelta(seconds=2)).strftime("%Y-%m-%d %H:%M:%S")
    t0 = time.time()
    p = subprocess.Popen(["perl", "-e", "alarm shift; exec @ARGV", str(timeout), "/bin/sh", "-c", f"{PROBE} apply {path} -- {cmd}"],
                         cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    pid_lo = p.pid
    out, _ = p.communicate()
    dt = time.time() - t0
    pid_hi = pid_lo
    m = re.search(r"\d+", sh("/bin/sh -c 'echo $$'").stdout)
    if m:
        pid_hi = int(m.group(0))
    return p.returncode, out, dt, start, pid_lo, pid_hi


def read_log(start):
    q = f"log show --start '{start}' --style compact --info --predicate 'process == \"kernel\" AND eventMessage CONTAINS \"Sandbox:\"'"
    text = sh(q).stdout
    if "deny(" not in text:
        text = sh("sudo -n " + q).stdout or text
    return text


def denials(start, lo, hi):
    """Our denials from the unified log, waiting for its lag until the count is stable."""
    best, prev = [], -1
    for i in range(5):
        time.sleep(3)
        out = []
        for line in read_log(start).splitlines():
            m = DENY.search(line)
            if not m:
                continue
            proc, pid, op, arg = m.group(1), int(m.group(2)), m.group(3), m.group(4).strip()
            if OURS.match(proc) or (hi >= lo and lo <= pid <= hi):
                out.append((proc, pid, op, arg))
        best = out
        if len(out) == prev and (out or i >= 2):
            break
        prev = len(out)
    return best


def attempt(items, kind, variant, shape=True, want_log=True):
    rc, out, dt, start, lo, hi = run_profile(profile_text(items, shape), kind, variant)
    ok = passed(rc, out, kind)
    dens = [] if (ok or not want_log) else denials(start, lo, hi)
    return ok, rc, out, dt, dens


def tail(out, n=500):
    return out[-n:].replace("\n", " | ")


# ---- the iterative loop --------------------------------------------------------------
def loop(items, kind, variant, tag, shape=True, max_rounds=40, show_all=False):
    seen = {(i.op, i.arg) for i in items}
    unhandled = collections.OrderedDict()
    for rnd in range(1, max_rounds + 1):
        if left() < 120:
            say(f"{tag} round {rnd}: time budget exhausted; stopping")
            return False, items, unhandled
        ok, rc, out, dt, dens = attempt(items, kind, variant, shape)
        say(f"{tag} round {rnd}: exit={rc} {dt:.0f}s; denials(ours) {len(dens)}; items {len(items)}")
        if ok:
            fin = [l.strip() for l in out.splitlines() if re.search(r"Finished|test result|running \d+ test|test t::", l)]
            say(f"{tag} RESULT: passed in round {rnd}: " + " | ".join(fin[:6]))
            return True, items, unhandled
        if rnd == 1 or rnd % 4 == 0 or show_all:
            say(f"{tag} raw denials: " + " ;; ".join(sorted({f'{p} {o} {a}' for p, _, o, a in dens}))[:3000])
        new = 0
        for proc, pid, op, arg in dens:
            key = (op, arg)
            if key in seen:
                continue
            seen.add(key)
            klass, alts = rule_candidates(op, arg)
            rule = None
            for alt in alts:
                if klass in ("file", "mach") or parses(alt):
                    rule = alt
                    break
            if rule is None:
                unhandled[key] = proc
                continue
            items.append(Item(klass, rule, op, arg, proc))
            new += 1
        say(f"{tag} round {rnd}: output tail: " + tail(out, 400 if new else 1500))
        if new == 0:
            say(f"{tag}: no new rule; stopping (unhandled denials: {len(unhandled)})")
            break
    return False, items, unhandled


def list_items(tag, items):
    for i in items:
        say(f"{tag} item [{i.kind}] {i.rule}  (first denied for: {i.who})")


# ---- necessity ----------------------------------------------------------------------
def group_of(i):
    if i.kind == "mach":
        return "mach " + i.arg
    if i.kind in ("net", "other"):
        return f"{i.kind} {i.op} {i.arg}"
    if i.kind == "seed":
        return "seed " + i.arg if DEV and i.arg == DEV else None  # only the developer directory is probed
    p = i.arg
    if p.startswith(HOME + "/") or p == HOME:
        return "home " + p
    if p.startswith("/System/Volumes/Preboot/Cryptexes"):
        return "group dyld-cache (/System/Volumes/Preboot/Cryptexes)"
    if p.startswith("/System/"):
        return "group /System (frameworks, logging prefs)"
    if p.startswith("/private/etc"):
        return "group /private/etc"
    if p.startswith("/dev/"):
        return "dev " + p
    if p.startswith("/opt/homebrew"):
        return "group /opt/homebrew (rustup and its libraries)"
    if p.startswith("/Library/"):
        return "group /Library/Preferences"
    if p == "/" or p == "/bin" or p.startswith("/bin/"):
        return "group / and /bin"
    return "group other " + os.path.dirname(p)


def without(items, pred):
    return [i for i in items if not pred(i)]


def necessity(items, kind, variant, tag, shape=True, passes=("A", "B")):
    """A: remove each unit (mach service, home file, other group) from the whole result, one at a time.
    B: greedy removal of every literal item, which yields a 1-minimal set."""
    base_ok = []
    for n in (1, 2):
        ok, rc, out, dt, _ = attempt(items, kind, variant, shape, want_log=False)
        base_ok.append(ok)
    say(f"{tag} necessity baseline (full result) run twice: {base_ok}")
    if not all(base_ok):
        say(f"{tag} necessity: the full result is not stable; skipping")
        return items
    if "A" in passes:
        groups = collections.OrderedDict()
        for i in items:
            g = group_of(i)
            if g:
                groups.setdefault(g, []).append(i)
        say(f"{tag} necessity A: {len(groups)} units")
        for g, members in groups.items():
            if left() < 240:
                say(f"{tag} necessity A: time budget exhausted before: {g}")
                break
            ids = {id(m) for m in members}
            ok, rc, out, dt, _ = attempt(without(items, lambda x: id(x) in ids), kind, variant, shape, want_log=False)
            say(f"{tag} necessity A | {'STILL PASSES (not needed alone)' if ok else 'FAILS (needed)'} | {g} | {len(members)} rule(s) | {dt:.0f}s | exit {rc}")
    cur = list(items)
    if "B" in passes:
        say(f"{tag} necessity B (greedy, {len(cur)} items)")
        for i in list(cur):
            if i.kind == "seed" and not (DEV and i.arg == DEV):
                continue
            if left() < 240:
                say(f"{tag} necessity B: time budget exhausted before: {i.label}")
                break
            trial = [x for x in cur if x is not i]
            ok, rc, out, dt, _ = attempt(trial, kind, variant, shape, want_log=False)
            say(f"{tag} necessity B | {'dropped' if ok else 'KEPT (needed)'} | {i.rule}")
            if ok:
                cur = trial
        confirm = []
        for n in (1, 2):
            ok, rc, out, dt, _ = attempt(cur, kind, variant, shape, want_log=False)
            confirm.append(ok)
        say(f"{tag} minimal set confirmed twice: {confirm}; {len(cur)} rules (of {len(items)})")
    return cur


def print_profile(tag, items, shape=True):
    for line in profile_text(items, shape).splitlines():
        say(f"{tag} profile| {line}")


def save(variant, kind, items):
    json.dump(dict(variant=variant, items=[i.to_json() for i in items]), open(f"{S}/{kind}-{variant}.json", "w"))


def load(path):
    d = json.load(open(path))
    return d["variant"], [Item(x["kind"], x["rule"], x["op"], x["arg"], x["who"], x["label"]) for x in d["items"]]


# ---- B3 -----------------------------------------------------------------------------
def b3_variant(variant, full):
    tag = f"b3.{variant}"
    say(f"{tag} ===== iterative loop, shape=Amendment 2, developer directory in the seed: {DEV} =====")
    ok, items, unhandled = loop(seeds(variant), "test", variant, tag)
    for (op, arg), proc in list(unhandled.items())[:60]:
        say(f"{tag} unhandled denial {op} {arg} ({proc})")
    list_items(tag + ".loop", items)
    if not ok:
        say(f"{tag} did NOT reach a passing run; trying the plain profile (no Amendment shape) to see whether the shape is the cause")
        ok2, items2, unh2 = loop(seeds(variant), "test", variant, tag + ".plain", shape=False, max_rounds=25)
        list_items(tag + ".plain.loop", items2)
        say(f"{tag}.plain result: {ok2}")
        return None
    save(variant, "loop", items)
    if full:
        # candidate: the read set of Amendment 2 point 8 plus whatever the loop found beyond it
        sysdirs = ["/usr", "/bin", "/sbin", "/System", "/private/etc"]
        cand = seeds(variant)
        for d in ["/bin", "/sbin", "/System", "/private/etc"]:
            cand.append(Item("seed", f'(allow file-read* (subpath "{d}"))', "seed", d, "amendment-2", f"amendment-2 subpath {d}"))
        extra = [i for i in items if i.kind != "seed" and not (i.kind == "file" and any(i.arg == d or i.arg.startswith(d + "/") for d in sysdirs + ([DEV] if DEV else [])))]
        cand += extra
        say(f"{tag} ===== candidate A: Amendment 2's system part (/usr /bin /sbin /System /private/etc, developer dir) + the loop's other items ({len(extra)}) =====")
        okc, rc, out, dt, dens = attempt(cand, "test", variant)
        say(f"{tag} candidate A passes: {okc}")
        if not okc:
            okc, cand, unh = loop(cand, "test", variant, tag + ".candA", max_rounds=8)
            say(f"{tag} candidate A after continuing: {okc}; additions beyond the Amendment's part: " + " ;; ".join(i.rule for i in cand if i.who not in ("seed", "amendment-2")))
        # the narrow developer directory experiment
        if DEV:
            narrow = []
            for sub in ("Toolchains/XcodeDefault.xctoolchain", "Platforms/MacOSX.platform", "usr", "SDKs", "Library"):
                p = os.path.realpath(f"{DEV}/{sub}")
                if os.path.exists(p):
                    narrow.append(p)
            trial = [i for i in items if not (i.kind == "seed" and i.arg == DEV)]
            trial += [Item("seed", f'(allow file-read* (subpath "{esc(p)}"))', "seed", p, "narrow-dev", f"narrow dev subpath {p}") for p in narrow]
            trial.append(Item("seed", f'(allow file-read* (literal "{esc(DEV)}"))', "seed", DEV + "(literal)", "narrow-dev"))
            oknd, rc, out, dt, dens = attempt(trial, "test", variant, want_log=True)
            say(f"{tag} narrow developer directory ({', '.join(narrow)}) passes: {oknd}; denials: " + " ;; ".join(sorted({f'{p} {o} {a}' for p, _, o, a in dens}))[:1500])
    say(f"{tag} ===== necessity =====")
    cur = necessity(items, "test", variant, tag, passes=("A", "B") if full else ("B",))
    save(variant, "minimal", cur)
    print_profile(tag + ".minimal", cur)
    return cur


# ---- B5 -----------------------------------------------------------------------------
def b5():
    tag = "b5"
    src = None
    for v in ("direct", "shim"):
        if os.path.exists(f"{S}/minimal-{v}.json"):
            src = f"{S}/minimal-{v}.json"
            break
    if src is None:
        for v in ("direct", "shim"):
            if os.path.exists(f"{S}/loop-{v}.json"):
                src = f"{S}/loop-{v}.json"
                break
    if src is None:
        say(f"{tag}: no base profile from B3 in this run (B3 did not converge); using the seed only")
        variant, base = ("shim" if "shim" in VARIANTS else "direct"), seeds("shim" if "shim" in VARIANTS else "direct")
    else:
        variant, base = load(src)
        say(f"{tag}: base profile = {os.path.basename(src)} ({len(base)} rules), variant {variant}")
    sibling = list(base)
    say(f"{tag} ===== the sibling (base profile, no network addition): cargo fetch =====")
    ok, rc, out, dt, dens = attempt(sibling, "fetch", variant)
    say(f"{tag}.sibling cargo fetch passes: {ok} (exit {rc}); output: " + tail(out, 1200))
    say(f"{tag}.sibling denials(ours): " + " ;; ".join(sorted({f'{p} {o} {a}' for p, _, o, a in dens}))[:2500])
    open(f"{S}/prof/b5-sibling.sb", "w").write(profile_text(sibling))
    r = subprocess.run(["perl", "-e", "alarm shift; exec @ARGV", "30", PROBEB, "resolve", f"{S}/prof/b5-sibling.sb", "index.crates.io"], capture_output=True, text=True)
    say(f"{tag}.sibling resolve: " + (r.stdout + r.stderr).replace("\n", " | "))
    say(f"{tag} ===== iterative loop for the additions (cargo fetch) =====")
    nbase = len(base)
    ok, items, unhandled = loop(list(base), "fetch", variant, tag, show_all=True, max_rounds=30)
    adds = items[nbase:]
    for (op, arg), proc in list(unhandled.items())[:60]:
        say(f"{tag} unhandled denial {op} {arg} ({proc})")
    say(f"{tag} the loop added {len(adds)} rules; fetch passes: {ok}")
    for i in adds:
        say(f"{tag} added [{i.kind}] {i.rule}  (first denied for: {i.who})")
    if not ok:
        return
    # necessity of the additions: one at a time, then greedy
    for n in (1, 2):
        okb, rc, out, dt, _ = attempt(items, "fetch", variant, want_log=False)
        say(f"{tag} baseline run {n} with all additions: {okb}")
    for i in adds:
        trial = [x for x in items if x is not i]
        okt, rc, out, dt, _ = attempt(trial, "fetch", variant, want_log=False)
        say(f"{tag} necessity A | {'STILL PASSES (not needed alone)' if okt else 'FAILS (needed)'} | {i.rule}")
    cur = list(items)
    for i in adds:
        trial = [x for x in cur if x is not i]
        okt, rc, out, dt, _ = attempt(trial, "fetch", variant, want_log=False)
        say(f"{tag} necessity B | {'dropped' if okt else 'KEPT (needed)'} | {i.rule}")
        if okt:
            cur = trial
    conf = [attempt(cur, "fetch", variant, want_log=False)[0] for _ in (1, 2)]
    say(f"{tag} smallest addition confirmed twice: {conf}; additions kept: {len(cur) - nbase}")
    for i in cur[nbase:]:
        say(f"{tag} smallest addition| {i.rule}")
    open(f"{S}/prof/b5-final.sb", "w").write(profile_text(cur))
    r = subprocess.run(["perl", "-e", "alarm shift; exec @ARGV", "30", PROBEB, "resolve", f"{S}/prof/b5-final.sb", "index.crates.io"], capture_output=True, text=True)
    say(f"{tag}.final resolve: " + (r.stdout + r.stderr).replace("\n", " | "))
    print_profile(tag + ".final", cur)


def main():
    diag()
    todo = WHAT.split(",")
    both = "b3" in todo and "b5" in todo
    if "b3" in todo:
        for variant in [v for v in ("shim", "direct") if v in VARIANTS]:
            phase_until((2400 if variant == "shim" else 3300) if both else 4300)
            if left() < 600:
                say(f"b3.{variant}: skipped, time budget")
                continue
            try:
                cur = b3_variant(variant, full=(variant == "shim"))
                if cur is not None:
                    save(variant, "minimal", cur)
            except Exception:
                say(f"b3.{variant} EXCEPTION: " + traceback.format_exc().replace("\n", " | "))
    if "b5" in todo:
        phase_until(4500)
        try:
            b5()
        except Exception:
            say("b5 EXCEPTION: " + traceback.format_exc().replace("\n", " | "))
    say(f"done in {time.time() - T0:.0f}s")


main()
