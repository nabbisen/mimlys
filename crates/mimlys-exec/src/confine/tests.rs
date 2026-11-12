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
        args: vec!["-c".into(), "sleep 120 & echo $! ; wait".into()],
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
    if cfg!(target_os = "linux") {
        // Decided by the process's state, not by a fixed pause: the grandchild
        // sleeps for 120 s, so it is gone because of the kill (never because
        // it ended), and a slow machine is given up to 60 s to deliver the signal.
        let alive = || {
            std::fs::read_to_string(format!("/proc/{grandchild}/stat"))
                .map(|s| !s.contains(") Z "))
                .unwrap_or(false)
        };
        let started = std::time::Instant::now();
        while alive() && started.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(), "the grandchild must be gone with the group");
    }
}

/// REQ-WS-001 (external design §14.1), Unix: the data directory and every
/// parent the call creates are mode 0700 from the start (the mode is given to
/// `mkdir`; for a directory that is new nothing is applied afterwards), a
/// subdirectory made through `create_private_subdir` is 0700 too, and a
/// directory that already existed with a wider mode is brought to 0700 at
/// the call, as at every open.
#[cfg(unix)]
#[test]
fn req_ws_001_unix_directories_are_0700_from_the_start() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("a/b/data");
    create_private_dir(&data).unwrap();
    for dir in [
        &data,
        data.parent().unwrap(),
        data.parent().unwrap().parent().unwrap(),
    ] {
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
    let sub = data.join("workspaces");
    create_private_subdir(&sub).unwrap();
    assert_eq!(mode(&sub), 0o700);
    // An existing directory with a wider mode is brought to 0700, children untouched.
    let existing = root.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(existing.join("f"), "x").unwrap();
    create_private_dir(&existing).unwrap();
    assert_eq!(mode(&existing), 0o700);
    assert!(existing.join("f").is_file());
    // A second call changes nothing.
    create_private_dir(&data).unwrap();
    assert_eq!(mode(&data), 0o700);
}

/// The entries `icacls` lists for a path: one string per access-control
/// entry, as printed (`DOMAIN\user:(OI)(CI)(F)`), the path itself removed
/// from the first line.
#[cfg(windows)]
fn icacls_entries(path: &std::path::Path) -> Vec<String> {
    let out = std::process::Command::new("icacls")
        .arg(path)
        .output()
        .expect("icacls runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let shown = path.to_string_lossy().into_owned();
    text.lines()
        .map(|l| {
            l.strip_prefix(shown.as_str())
                .unwrap_or(l)
                .trim()
                .to_owned()
        })
        .filter(|l| l.contains(":(") && !l.starts_with("Successfully"))
        .collect()
}

/// The current user as `icacls` prints it: `whoami`, which is `DOMAIN\user`.
#[cfg(windows)]
fn whoami() -> String {
    let out = std::process::Command::new("whoami").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_lowercase()
}

/// REQ-WS-001 (external design §14.1), Windows: `create_private_dir` gives
/// the directory a protected DACL with one entry, the current user's full
/// control, inheritable; the current user can still create, read and delete
/// inside it (nothing locks mimlys out); a directory made beneath it inherits
/// that one entry and no other; a second call changes nothing; and a
/// directory that had other entries (an inherited set, an added one) loses
/// them at the call.
#[cfg(windows)]
#[test]
fn req_ws_001_windows_dacl_is_the_current_user_alone() {
    let me = whoami();
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    create_private_dir(&data).unwrap();
    let only_me = |path: &std::path::Path| {
        let entries = icacls_entries(path);
        assert_eq!(entries.len(), 1, "{}: {entries:?}", path.display());
        let entry = entries[0].to_lowercase();
        assert!(entry.starts_with(&format!("{me}:")), "{entry}, user {me}");
        assert!(entry.contains("(f)"), "full control: {entry}");
        entry
    };
    let entry = only_me(&data);
    assert!(entry.contains("(oi)") && entry.contains("(ci)"), "{entry}");
    assert!(
        !entry.contains("(i)"),
        "nothing is inherited into the data directory: {entry}"
    );
    // Nothing locks the user out.
    std::fs::write(data.join("f.txt"), "x").unwrap();
    assert_eq!(std::fs::read_to_string(data.join("f.txt")).unwrap(), "x");
    // A subdirectory inherits the one entry.
    let sub = data.join("workspaces");
    create_private_subdir(&sub).unwrap();
    let inherited = only_me(&sub);
    assert!(
        inherited.contains("(i)"),
        "inherited from the data directory: {inherited}"
    );
    std::fs::write(sub.join("g.txt"), "y").unwrap();
    std::fs::remove_file(sub.join("g.txt")).unwrap();
    // A second call writes nothing new, and the result is the same.
    create_private_dir(&data).unwrap();
    only_me(&data);
    // Another entry (Users: read) is removed at the next call.
    let status = std::process::Command::new("icacls")
        .arg(&data)
        .args(["/grant", "*S-1-5-32-545:(OI)(CI)(R)"])
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(icacls_entries(&data).len(), 2);
    create_private_dir(&data).unwrap();
    only_me(&data);
    assert_eq!(std::fs::read_to_string(data.join("f.txt")).unwrap(), "x");
}
