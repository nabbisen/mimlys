use super::*;

/// REQ-CONF-009, REQ-TEST-013 (handoff 011g): the `/proc` walk finds the
/// descendants of a process, a grandchild included, and kills them within the
/// grace period (clause K's fallback rung). The walk is rooted at a helper this
/// test starts, never at the test process: other tests of the same binary
/// start children of it in parallel threads, and a walk from the test process
/// would see, and kill, those. In the product the root is the worker, whose
/// only children are its tools. The helper has two children, one of which has
/// a child of its own (helper → inner `sh` → `sleep`, helper → `sleep`), so the
/// walk's following of ancestry beyond one level is exercised; the trailing `:`
/// keeps the inner shell from replacing itself with its `sleep`.
#[test]
fn rfc_011_descendant_walk_kills_the_tree() {
    let mut helper = std::process::Command::new("sh")
        .args(["-c", "sh -c 'sleep 120; :' & sleep 120 & wait"])
        .spawn()
        .unwrap();
    let root = helper.id();
    // All three exist before the walk (a state, awaited up to 60 s). The sleepers hold
    // for 120 s, far longer than the 60 s the wait and the kill are given.
    let started = std::time::Instant::now();
    while descendants_of(root).len() < 3 {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the helper's tree did not start: {:?}",
            descendants_of(root)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let found = descendants_of(root);
    assert_eq!(
        found.len(),
        3,
        "inner shell, its sleep, the sleep: {found:?}"
    );
    assert!(
        !found.contains(&root),
        "the root is not its own descendant: {found:?}"
    );
    let parent_of = |pid: u32| ppid_of(Path::new(&format!("/proc/{pid}")));
    let children = found
        .iter()
        .filter(|p| parent_of(**p) == Some(root))
        .count();
    let grandchildren: Vec<u32> = found
        .iter()
        .copied()
        .filter(|p| parent_of(*p).is_some_and(|pp| pp != root && found.contains(&pp)))
        .collect();
    assert_eq!(children, 2, "two children of the helper: {found:?}");
    assert_eq!(
        grandchildren.len(),
        1,
        "one grandchild, found by following the ancestry: {found:?}"
    );
    let remaining = kill_descendants(root, Duration::from_secs(60));
    assert_eq!(remaining, 0);
    // Gone now, not when the sleepers would have ended on their own, the
    // grandchild included.
    assert!(
        descendants_of(root).is_empty(),
        "left after the kill: {:?}",
        descendants_of(root)
    );
    assert!(
        !Path::new(&format!("/proc/{}", grandchildren[0])).exists()
            || parent_of(grandchildren[0]).is_none(),
        "the grandchild is gone"
    );
    let _ = helper.wait();
}

/// The stat parser copes with a comm that has spaces and parentheses.
#[test]
fn ppid_parser_handles_odd_comm() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("stat"),
        "123 (my (odd) prog) S 77 123 123 0 -1 4194304\n",
    )
    .unwrap();
    assert_eq!(ppid_of(dir.path()), Some(77));
    std::fs::write(
        dir.path().join("stat"),
        "124 (gone) Z 77 123 123 0 -1 4194304\n",
    )
    .unwrap();
    assert_eq!(
        ppid_of(dir.path()),
        None,
        "a zombie is not a descendant to kill"
    );
}

fn spec_denying_network(tmp: &Path) -> ConfinementSpec {
    ConfinementSpec {
        read_paths: vec![
            PathBuf::from("/usr"),
            PathBuf::from("/lib"),
            PathBuf::from("/lib64"),
        ],
        write_paths: vec![tmp.to_path_buf()],
        deny_network: true,
        memory_bytes: 1 << 30,
        max_processes: 64,
        data_dir: tmp.to_path_buf(),
    }
}

/// REQ-CONF-012: where Landlock has the TCP layer (ABI 4), the worker's
/// ruleset denies TCP `connect` and `bind` by itself, without the seccomp
/// filter (none is installed in this process). The ruleset is applied to one
/// thread only, which is how Landlock restricts; the control, a connect from
/// the thread that is not restricted, succeeds to the same listener.
#[test]
fn req_conf_012_landlock_tcp_layer_denies_connect_and_bind() {
    let Some(abi) = detect_abi() else {
        crate::test_skip::skip(
            "req_conf_012_landlock_tcp_layer_denies_connect_and_bind",
            "no Landlock on this kernel",
        );
        return;
    };
    if abi < ABI::V4 {
        crate::test_skip::skip(
            "req_conf_012_landlock_tcp_layer_denies_connect_and_bind",
            "Landlock ABI below 4",
        );
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    assert!(
        std::net::TcpStream::connect(addr).is_ok(),
        "the control connects"
    );
    let spec = spec_denying_network(tmp.path());
    let result = std::thread::spawn(move || {
        let (ruleset, net) = landlock_ruleset(&spec, None, abi, &[], true).expect("ruleset");
        assert!(net, "the TCP layer is part of the worker's ruleset");
        ruleset.restrict_self().expect("restrict this thread");
        let connect = std::net::TcpStream::connect(addr)
            .map(|_| ())
            .map_err(|e| e.kind());
        let bind = std::net::TcpListener::bind("127.0.0.1:0")
            .map(|_| ())
            .map_err(|e| e.kind());
        (connect, bind)
    })
    .join()
    .unwrap();
    assert_eq!(
        result.0,
        Err(std::io::ErrorKind::PermissionDenied),
        "connect"
    );
    assert_eq!(result.1, Err(std::io::ErrorKind::PermissionDenied), "bind");
    // The dependency step's ruleset (network allowed) leaves TCP alone.
    let mut open = spec_denying_network(tmp.path());
    open.deny_network = false;
    let allowed = std::thread::spawn(move || {
        let (ruleset, net) = landlock_ruleset(&open, None, abi, &[], true).expect("ruleset");
        assert!(!net);
        ruleset.restrict_self().expect("restrict this thread");
        std::net::TcpStream::connect(addr)
            .map(|_| ())
            .map_err(|e| e.kind())
    })
    .join()
    .unwrap();
    assert_eq!(allowed, Ok(()), "network allowed for the dependency step");
}

/// REQ-CONF-012: the deny table holds the five syscalls the amendment names.
/// The table is applied whether or not network is denied (`install_seccomp`
/// inserts every entry before it looks at `deny_network`), so pinning the
/// table pins both workers, the dependency step's included.
#[test]
fn req_conf_012_always_denied_syscalls_include_io_uring_and_vm_access() {
    for (name, nr) in [
        ("io_uring_setup", libc::SYS_io_uring_setup),
        ("io_uring_enter", libc::SYS_io_uring_enter),
        ("io_uring_register", libc::SYS_io_uring_register),
        ("process_vm_readv", libc::SYS_process_vm_readv),
        ("process_vm_writev", libc::SYS_process_vm_writev),
        ("ptrace", libc::SYS_ptrace),
    ] {
        assert!(ALWAYS_DENIED.contains(&nr), "{name}");
    }
}

/// REQ-CONF-005: F and N say `enforced` only when a primitive keeps a Unix
/// socket inside the write set, and name it: Landlock's `ResolveUnix` where the
/// ABI has it and the ruleset held in full, the filter's denial of
/// `socket(AF_UNIX, ..)` where it does not and the filter was installed.
#[test]
fn req_conf_005_clauses_are_enforced_only_when_a_unix_socket_guard_holds() {
    let full = || {
        Ok(Landlock {
            status: RulesetStatus::FullyEnforced,
            net: true,
        })
    };
    // ABI 9 and later: the guard is Landlock's, whatever the filter did.
    assert_eq!(unix_guard(true, true, true), Some("landlock_resolve_unix"));
    assert_eq!(unix_guard(true, true, false), Some("landlock_resolve_unix"));
    // The ruleset did not hold in full: the filter does not stand in for it
    // on this ABI, where it does not deny the socket.
    assert_eq!(unix_guard(true, false, true), None);
    // Below ABI 9: the filter's, when it was installed, and only then.
    assert_eq!(
        unix_guard(false, true, true),
        Some("seccomp_no_unix_socket")
    );
    assert_eq!(unix_guard(false, true, false), None);
    assert_eq!(unix_guard(false, false, false), None);

    assert_eq!(
        filesystem_state(&full(), unix_guard(true, true, true)),
        ClauseState::enforced("landlock_resolve_unix")
    );
    assert_eq!(
        filesystem_state(&full(), unix_guard(false, true, true)),
        ClauseState::enforced("landlock+seccomp_no_unix_socket")
    );
    assert_eq!(
        filesystem_state(&full(), None),
        ClauseState::unavailable("no_unix_socket_guard"),
        "a complete ruleset without the guard is not enforced"
    );
    assert!(matches!(
        filesystem_state(&Err("landlock_unsupported".into()), None),
        ClauseState::Unavailable { .. }
    ));

    assert_eq!(
        network_state(true, true, true, Some("landlock_resolve_unix")),
        ClauseState::enforced("seccomp+landlock_net+landlock_resolve_unix")
    );
    assert_eq!(
        network_state(true, true, false, Some("seccomp_no_unix_socket")),
        ClauseState::enforced("seccomp+seccomp_no_unix_socket")
    );
    assert_eq!(
        network_state(true, true, true, None),
        ClauseState::unavailable("no_unix_socket_guard")
    );
    assert_eq!(
        network_state(true, false, false, Some("landlock_resolve_unix")),
        ClauseState::unavailable("seccomp_unsupported")
    );
    assert!(matches!(
        network_state(false, true, false, Some("landlock_resolve_unix")),
        ClauseState::Unavailable { .. }
    ));
}

/// What a thread restricted by the filter finds when it creates sockets.
#[derive(Debug, PartialEq, Eq)]
struct Sockets {
    /// `socket(AF_UNIX, ..)`: a listener bound in a directory of the test.
    unix: Result<(), std::io::ErrorKind>,
    /// `socketpair(AF_UNIX, ..)`.
    pair: Result<(), std::io::ErrorKind>,
    /// `socket(AF_INET, ..)`: a TCP listener on the loopback.
    inet: Result<(), std::io::ErrorKind>,
}

/// Installs the filter of the given shape on one thread (a filter binds the
/// thread that installs it and what it starts) and tries each socket call.
fn sockets_under_filter(deny_network: bool, deny_unix: bool) -> Sockets {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sock");
    std::thread::spawn(move || {
        let program = seccomp_program(deny_network, deny_unix).expect("filter");
        seccompiler::apply_filter(&program).expect("install");
        let kind = |r: std::io::Result<()>| r.map_err(|e| e.kind());
        Sockets {
            unix: kind(std::os::unix::net::UnixListener::bind(&path).map(|_| ())),
            pair: kind(std::os::unix::net::UnixStream::pair().map(|_| ())),
            inet: kind(std::net::TcpListener::bind("127.0.0.1:0").map(|_| ())),
        }
    })
    .join()
    .unwrap()
}

/// REQ-CONF-005: below Landlock ABI 9 the filter denies `socket(AF_UNIX, ..)`
/// and keeps `socketpair`; with network denied it denies every other family
/// too, and with network allowed (the dependency step) only `AF_UNIX`.
#[test]
fn req_conf_005_the_filter_denies_unix_sockets_below_landlock_abi_9() {
    let denied = Err(std::io::ErrorKind::PermissionDenied);
    // Network denied, below ABI 9: no socket call at all, the pair apart.
    assert_eq!(
        sockets_under_filter(true, true),
        Sockets {
            unix: denied,
            pair: Ok(()),
            inet: denied
        }
    );
    // Network allowed (the dependency step), below ABI 9: only Unix sockets.
    assert_eq!(
        sockets_under_filter(false, true),
        Sockets {
            unix: denied,
            pair: Ok(()),
            inet: Ok(())
        }
    );
    // ABI 9 and later: Landlock keeps Unix sockets in the write set, so the
    // filter leaves them to it; the network is denied as before.
    assert_eq!(
        sockets_under_filter(true, false),
        Sockets {
            unix: Ok(()),
            pair: Ok(()),
            inet: denied
        }
    );
}

/// REQ-CONF-005: the ABI the backend asks for follows the cap, so that the
/// rung below ABI 9 runs on any kernel, and never exceeds the kernel's.
#[test]
fn req_conf_005_the_abi_cap_selects_the_rung() {
    let kernel = detect_abi_capped(None);
    let capped = detect_abi_capped(Some(8));
    if let Some(capped) = capped {
        assert!(capped < ABI::V9, "{capped:?}");
        assert!(Some(capped) <= kernel);
    }
    assert_eq!(detect_abi_capped(Some(0)), None);
    assert!(detect_abi_capped(Some(100)) == kernel);
}

/// REQ-CONF-010: the host tools' layer holds no right on the worker's own
/// cgroup directory, which the worker's layer does hold. The directory is a
/// stand-in outside the write set, with the file the cgroup filesystem would
/// have; each layer is applied to a thread of its own.
#[test]
fn req_conf_010_the_tool_layer_holds_no_right_on_the_workers_cgroup() {
    let Some(abi) = detect_abi_capped(None) else {
        crate::test_skip::skip(
            "req_conf_010_the_tool_layer_holds_no_right_on_the_workers_cgroup",
            "no Landlock on this kernel",
        );
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let cgroup = tmp.path().join("mimlys-1");
    std::fs::create_dir_all(cgroup.join(TOOL_CGROUP)).unwrap();
    let procs = cgroup.join(TOOL_CGROUP).join("cgroup.procs");
    std::fs::write(&procs, "").unwrap();
    let write_set = tmp.path().join("write");
    std::fs::create_dir_all(&write_set).unwrap();
    let own = write_set.join("own");
    std::fs::write(&own, "").unwrap();
    let mut spec = spec_denying_network(tmp.path());
    spec.write_paths = vec![write_set];
    let open_for_write = |path: &Path| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map(|_| ())
            .map_err(|e| e.kind())
    };
    // Each layer is made for its own thread: a ruleset is applied once.
    let apply = |worker: bool| {
        let layers = build_layers(&spec, Some(&cgroup), abi).expect("layers");
        let (procs, own) = (procs.clone(), own.clone());
        std::thread::spawn(move || {
            let layer = if worker { layers.worker } else { layers.tool };
            layer.restrict_self().expect("restrict this thread");
            (open_for_write(&procs), open_for_write(&own))
        })
        .join()
        .unwrap()
    };
    let (tool_procs, tool_own) = apply(false);
    assert_eq!(
        tool_procs,
        Err(std::io::ErrorKind::PermissionDenied),
        "the tool layer has no right on the worker's cgroup"
    );
    assert_eq!(tool_own, Ok(()), "and still writes its own set");
    let (worker_procs, _) = apply(true);
    assert_eq!(worker_procs, Ok(()), "the worker's layer has it");
}
