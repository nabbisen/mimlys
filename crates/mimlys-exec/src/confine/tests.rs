use super::*;
use mimlys_core::event::Tier;

/// REQ-CONF-003: the M2 probe enforces E and K everywhere, P on Linux, and
/// names the milestone of every other clause; the tier is `core`.
#[test]
fn req_conf_003_m2_probe() {
    let probe = platform().probe();
    assert_eq!(probe.clauses[&Clause::Environment], ClauseState::ENFORCED);
    assert_eq!(probe.clauses[&Clause::Termination], ClauseState::ENFORCED);
    assert_eq!(probe.tier(), Tier::Core);
    for c in [
        Clause::Filesystem,
        Clause::Network,
        Clause::Memory,
        Clause::Count,
    ] {
        assert!(
            matches!(&probe.clauses[&c], ClauseState::Unavailable { reason } if reason.starts_with("backend arrives in M6")),
            "{c:?}"
        );
    }
    if cfg!(target_os = "linux") {
        assert_eq!(probe.clauses[&Clause::Privilege], ClauseState::ENFORCED);
    }
}

/// REQ-CONF-009 / clause K: killing the tree ends a child that spawned a grandchild.
#[cfg(unix)]
#[test]
fn req_conf_009_tree_termination() {
    let command = WorkerCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "sleep 30 & echo $! ; wait".into()],
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        current_dir: None,
    };
    let mut child = platform().spawn(&command, None).unwrap();
    let mut line = String::new();
    {
        use std::io::BufRead;
        let stdout = child.take_stdout().unwrap();
        std::io::BufReader::new(stdout)
            .read_line(&mut line)
            .unwrap();
    }
    let grandchild: u32 = line.trim().parse().unwrap();
    assert!(
        std::path::Path::new(&format!("/proc/{grandchild}")).exists()
            || cfg!(not(target_os = "linux"))
    );
    child.kill_tree();
    std::thread::sleep(Duration::from_millis(200));
    if cfg!(target_os = "linux") {
        let alive = std::fs::read_to_string(format!("/proc/{grandchild}/stat"))
            .map(|s| !s.contains(") Z "))
            .unwrap_or(false);
        assert!(!alive, "the grandchild must be gone with the group");
    }
}
