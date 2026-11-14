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

/// What a thread restricted by one layer finds when it opens a path for
/// writing, reads a file and lists a directory.
fn open_for_write(path: &Path) -> Result<(), std::io::ErrorKind> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map(|_| ())
        .map_err(|e| e.kind())
}

/// Applies `layer` to one thread and runs `probe` on it: a ruleset restricts
/// the thread that applies it, so the test process is left as it was.
fn on_restricted_thread<T: Send + 'static>(
    layer: landlock::RulesetCreated,
    probe: impl FnOnce() -> T + Send + 'static,
) -> T {
    std::thread::spawn(move || {
        layer.restrict_self().expect("restrict this thread");
        probe()
    })
    .join()
    .unwrap()
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
    let data = tmp.path().join("data");
    let task = data.join("workspaces").join("a");
    std::fs::create_dir_all(task.join("tree")).unwrap();
    std::fs::create_dir_all(task.join("tmp").join("e")).unwrap();
    let cgroup = tmp.path().join("mimlys-1");
    std::fs::create_dir_all(cgroup.join(TOOL_CGROUP)).unwrap();
    let procs = cgroup.join(TOOL_CGROUP).join("cgroup.procs");
    std::fs::write(&procs, "").unwrap();
    let mut spec = spec_denying_network(&data);
    spec.write_paths = vec![data.join("workspaces"), data.join("tmp")];
    let paths = TaskPaths {
        tree: &task.join("tree"),
        tmp: &task.join("tmp").join("e"),
    };
    let tool_layer = tool_ruleset(&spec, abi, paths).expect("the tool's layer");
    let probe_procs = procs.clone();
    let tool = on_restricted_thread(tool_layer, move || open_for_write(&probe_procs));
    assert_eq!(
        tool,
        Err(std::io::ErrorKind::PermissionDenied),
        "the tool layer has no right on the worker's cgroup"
    );
    // The worker's layer is made as `apply_landlock` makes it, with the cgroup.
    let (worker_layer, _) =
        landlock_ruleset(&spec, Some(&cgroup), abi, &WORKER_ONLY_READ, true).expect("worker");
    let worker = on_restricted_thread(worker_layer, move || open_for_write(&procs));
    assert_eq!(worker, Ok(()), "the worker's layer has it");
}

/// REQ-CONF-006: a host tool's layer holds, of the data directory, its own
/// task's workspace tree and the execution's temporary directory, and
/// nothing else: it can neither list, read nor write another task's tree,
/// nor the task's own metadata, nor the runtime's temporary directory (which
/// it may read), nor list the tasks; and it still writes its own.
#[test]
fn req_conf_006_a_tool_layer_holds_only_its_own_tasks_workspace() {
    let Some(abi) = detect_abi_capped(None) else {
        crate::test_skip::skip(
            "req_conf_006_a_tool_layer_holds_only_its_own_tasks_workspace",
            "no Landlock on this kernel",
        );
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let workspaces = data.join("workspaces");
    for task in ["a", "b"] {
        std::fs::create_dir_all(workspaces.join(task).join("tree")).unwrap();
        std::fs::create_dir_all(workspaces.join(task).join("tmp").join("e")).unwrap();
        std::fs::write(workspaces.join(task).join("tree").join("file"), task).unwrap();
        std::fs::write(workspaces.join(task).join("info.json"), "{}").unwrap();
    }
    std::fs::create_dir_all(data.join("tmp")).unwrap();
    std::fs::write(data.join("tmp").join("shared"), "x").unwrap();
    let mut spec = spec_denying_network(&data);
    spec.read_paths.push(data.join("tmp"));
    spec.write_paths = vec![workspaces.clone(), data.join("tmp")];
    let a = workspaces.join("a");
    let paths = TaskPaths {
        tree: &a.join("tree"),
        tmp: &a.join("tmp").join("e"),
    };
    let layer = tool_ruleset(&spec, abi, paths).expect("the tool's layer");
    let (a, b) = (workspaces.join("a"), workspaces.join("b"));
    let seen = on_restricted_thread(layer, move || {
        let read = |p: PathBuf| std::fs::read(p).map(|_| ()).map_err(|e| e.kind());
        let list = |p: PathBuf| std::fs::read_dir(p).map(|_| ()).map_err(|e| e.kind());
        [
            // Its own: tree and temporary directory, read and write.
            (
                "write own tree",
                open_for_write(&a.join("tree").join("file")),
            ),
            ("read own tree", read(a.join("tree").join("file"))),
            ("list own tree", list(a.join("tree"))),
            (
                "write own tmp",
                open_for_write(&a.join("tmp").join("e").join("new")).or_else(|e| {
                    // The file does not exist yet: creating it is the write.
                    std::fs::write(a.join("tmp").join("e").join("new"), "x").map_err(|_| e)
                }),
            ),
            // Another task's, and the rest of the data directory.
            (
                "write another tree",
                open_for_write(&b.join("tree").join("file")),
            ),
            ("read another tree", read(b.join("tree").join("file"))),
            ("list another tree", list(b.join("tree"))),
            (
                "create in another tree",
                std::fs::write(b.join("tree").join("new"), "x").map_err(|e| e.kind()),
            ),
            ("write own metadata", open_for_write(&a.join("info.json"))),
            ("list the tasks", list(a.parent().unwrap().to_path_buf())),
            (
                "write the runtime's tmp",
                std::fs::write(
                    a.parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("tmp")
                        .join("new"),
                    "x",
                )
                .map_err(|e| e.kind()),
            ),
            (
                "read the runtime's tmp",
                read(
                    a.parent()
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("tmp")
                        .join("shared"),
                ),
            ),
        ]
    });
    let denied = Err(std::io::ErrorKind::PermissionDenied);
    let expected: [(&str, Result<(), std::io::ErrorKind>); 12] = [
        ("write own tree", Ok(())),
        ("read own tree", Ok(())),
        ("list own tree", Ok(())),
        ("write own tmp", Ok(())),
        ("write another tree", denied),
        ("read another tree", denied),
        ("list another tree", denied),
        ("create in another tree", denied),
        ("write own metadata", denied),
        ("list the tasks", denied),
        ("write the runtime's tmp", denied),
        ("read the runtime's tmp", Ok(())),
    ];
    assert_eq!(seen, expected);
}

/// REQ-CONF-006: the layer is made only for what it says: the tree of one
/// task directly under `<data>/workspaces`, with a temporary directory inside
/// the same task's directory. Anything else is refused, never widened.
#[test]
fn req_conf_006_a_tool_layer_is_refused_for_paths_that_are_not_one_task() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let workspaces = data.join("workspaces");
    for task in ["a", "b"] {
        std::fs::create_dir_all(workspaces.join(task).join("tree")).unwrap();
        std::fs::create_dir_all(workspaces.join(task).join("tmp")).unwrap();
    }
    let mut spec = spec_denying_network(&data);
    spec.write_paths = vec![workspaces.clone(), data.join("tmp")];
    let (a_tree, a_tmp) = (workspaces.join("a/tree"), workspaces.join("a/tmp"));
    let ok = |tree: &Path, tmp: &Path| tool_spec(&spec, TaskPaths { tree, tmp });
    let granted = ok(&a_tree, &a_tmp).expect("one task");
    assert!(granted.write_paths.contains(&a_tree) && granted.write_paths.contains(&a_tmp));
    assert!(
        !granted
            .write_paths
            .iter()
            .any(|p| p.starts_with(&data) && p != &a_tree && p != &a_tmp),
        "nothing of the data directory but the two: {:?}",
        granted.write_paths
    );
    // The temporary directory of another task, a tree that is not a tree, a
    // task directory itself, a path outside the data directory.
    assert!(ok(&a_tree, &workspaces.join("b/tmp")).is_err());
    assert!(ok(&workspaces.join("a"), &a_tmp).is_err());
    assert!(ok(&workspaces, &a_tmp).is_err());
    assert!(ok(&tmp.path().join("elsewhere/tree"), &a_tmp).is_err());
    assert!(ok(&a_tree, &workspaces.join("a/tmp/missing")).is_err());
    let mut no_data_dir = spec.clone();
    no_data_dir.data_dir = PathBuf::new();
    assert!(
        tool_spec(
            &no_data_dir,
            TaskPaths {
                tree: &a_tree,
                tmp: &a_tmp
            }
        )
        .is_err()
    );
}
