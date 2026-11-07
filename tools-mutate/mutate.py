#!/usr/bin/env python3
"""THROWAWAY (branch 011c-1-mutations only; never merged). Mutation checks of
the macOS clause suite on macos-latest: each mutation removes or breaks one
rule of the backend; the named test must fail; the tree is restored.

Prints one block per mutation, lines starting with "MUT ".
"MUT <name>: killed by <tests>" is the wanted result;
"MUT <name>: SURVIVED" means no test failed (a finding)."""
import os, re, subprocess, sys, pathlib

root = pathlib.Path.cwd()
MACOS = "crates/mimlys-exec/src/confine/macos.rs"
HOST = "crates/mimlys-exec/src/host_tool.rs"
CONFINE = "crates/mimlys-exec/src/confine.rs"
WORKER = "crates/mimlys-exec/src/worker_main.rs"

# The self-check is part of the backend and sees most of these mutations
# (that is its job). STUB makes it report "held", so that the clause test
# is the only guard left; the "(self-check only)" variants run without it.
STUB = (MACOS,
        "    Outcome::Applied {\n        filesystem,\n        network,\n    }\n}",
        "    let _ = (filesystem, network);\n    Outcome::Applied {\n        filesystem: None,\n        network: None,\n    }\n}")

READ_ALL = (MACOS, 'p.push_str(&format!("(allow file-read*{})\\n", subpaths(&reads)));', 'p.push_str("(allow file-read*)\\n");')
WRITE_ALL = (MACOS, '"(allow file-read*{w})\\n(allow file-write*{w})\\n"', '"(allow file-read*{w})\\n(allow file-write*)\\n"')
NET_ALLOW = (MACOS, 'p.push_str("(deny network*)\\n");', 'p.push_str("(allow network*)\\n");')
DNS = (MACOS, "    Ok(p)\n}\n\n/// The profile for a specification as given",
       '    p.push_str("(allow mach-lookup (global-name \\"com.apple.dnssd.service\\") (global-name \\"com.apple.mDNSResponder\\"))\\n(allow network-outbound (remote unix-socket (literal \\"/private/var/run/mDNSResponder\\")))\\n");\n    Ok(p)\n}\n\n/// The profile for a specification as given')
UNIX_ANY = (MACOS, "    Ok(p)\n}\n\n/// The profile for a specification as given",
            '    p.push_str("(allow network-outbound (remote unix-socket))\\n");\n    Ok(p)\n}\n\n/// The profile for a specification as given')
PROCARGS = (MACOS, '(deny sysctl-read (sysctl-name-prefix \\"kern.procargs\\"))\\n', "")
PROCINFO = (MACOS, "(allow process-info* (target same-sandbox))", "(allow process-info*)")
SIGNAL = (MACOS, "(allow signal (target same-sandbox))", "(allow signal)")
NO_PROFILE = (MACOS, "sys::apply(init, &profile)", "Ok::<(), String>(())")
NO_CANON = (MACOS, "pub fn canonical(path: &Path) -> PathBuf {\n", "pub fn canonical(path: &Path) -> PathBuf {\n    if true {\n        return path.to_path_buf();\n    }\n")
K_ENFORCED = (MACOS, '("K".to_owned(), ClauseState::unavailable(K_LATER)),', '("K".to_owned(), ClauseState::enforced("process_group")),')
ENV_TOOL = (HOST, "        .env_clear()\n", "")
ENV_WORKER = (CONFINE, "            .env_clear()\n", "")
HONEST = (WORKER, 'std::env::var("MIMLYS_PROBE_FAIL")\n        && cfg!(debug_assertions)', 'std::env::var("MIMLYS_PROBE_FAIL_DISABLED")\n        && cfg!(debug_assertions)')

MUTATIONS = [
    ("F read set is every file", [READ_ALL, STUB], "req_conf_002_read_outside_the_set_is_denied"),
    ("F read set is every file (self-check only)", [READ_ALL], "req_conf_002_read_outside_the_set_is_denied"),
    ("F write set is every file", [WRITE_ALL], "req_conf_002_write_outside_the_workspace_is_denied"),
    ("N network is allowed", [NET_ALLOW, STUB], "req_conf_005_tcp_to_loopback_is_denied"),
    ("N network is allowed (self-check only)", [NET_ALLOW], "req_conf_005_tcp_to_loopback_is_denied"),
    ("N DNS services are allowed", [DNS], "req_conf_005_name_lookup_is_denied"),
    ("N Unix sockets are allowed anywhere", [UNIX_ANY], "req_conf_005_unix_sockets_only_under_the_write_set"),
    ("REQ-CONF-011 kern.procargs is readable", [PROCARGS, STUB], "req_conf_011_procargs_of_the_runtime_is_refused"),
    ("REQ-CONF-011 kern.procargs is readable (self-check only)", [PROCARGS], "req_conf_011_procargs_of_the_runtime_is_refused"),
    ("REQ-CONF-011 process information of others is allowed", [PROCINFO, STUB], "req_conf_011_other_processes_are_invisible"),
    ("REQ-CONF-011 process information of others is allowed (self-check only)", [PROCINFO], "req_conf_011_other_processes_are_invisible"),
    ("REQ-CONF-011 signals to others are allowed", [SIGNAL], "req_conf_011_other_processes_are_invisible"),
    ("P/F/N no profile is applied", [NO_PROFILE, STUB], "req_conf_005_setuid_exec_is_refused"),
    ("paths are not canonical", [NO_CANON], "req_conf_002_the_workspace_is_usable"),
    ("K is reported enforced without a mechanism", [K_ENFORCED], "req_conf_003_macos_status_table"),
    ("E the environment is inherited", [ENV_TOOL, ENV_WORKER], "req_conf_002_the_operators_secret_is_not_in_the_tools_environment"),
    ("probe ignores MIMLYS_PROBE_FAIL", [HONEST], "req_conf_003_probe_is_honest_and_fails_closed"),
]


def sh(cmd):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True, cwd=root)


only = sys.argv[1:]  # optional: names (substrings) to run
originals = {}
for name, edits, expected in MUTATIONS:
    if only and not any(o in name for o in only):
        continue
    ok = True
    for path, old, new in edits:
        text = originals.setdefault(path, (root / path).read_text())
        current = (root / path).read_text()
        if old not in current:
            print(f"MUT {name}: NOT APPLICABLE (text not found in {path}): {old[:60]!r}", flush=True)
            ok = False
            break
        (root / path).write_text(current.replace(old, new, 1))
    if ok and os.environ.get('DRY'):
        print(f'MUT {name}: applicable', flush=True)
    elif ok:
        r = sh("cargo test -p mimlys --all-features --test m6c_host_tools --no-fail-fast 2>&1")
        out = r.stdout
        failing = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)))
        if "error[" in out or "could not compile" in out:
            print(f"MUT {name}: DID NOT COMPILE\n{out[-1500:]}", flush=True)
        elif not failing:
            print(f"MUT {name}: SURVIVED (expected {expected})", flush=True)
        else:
            mark = "yes" if expected in failing else "NO"
            print(f"MUT {name}: killed by {', '.join(failing)}; the targeted test {expected} failed: {mark}", flush=True)
            m = re.search(r"---- %s stdout ----\n(.*?)\n\n" % re.escape(expected), out, re.S)
            if m:
                body = m.group(1).strip().splitlines()
                for line in body[:6]:
                    print("MUT     | " + line[:300], flush=True)
    for path, text in originals.items():
        (root / path).write_text(text)
