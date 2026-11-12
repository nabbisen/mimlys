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
