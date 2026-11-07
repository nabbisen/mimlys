//! Unit tests of the macOS backend's pure half (RFC 011 Amendment 2 points
//! 2 to 5). They run on every platform: the profile is text, and what it
//! must say is the specification's, not the implementation's. What the
//! kernel does with it is the clause suite's (`tests/m6c_host_tools.rs`).

use super::*;
use mimlys_core::confine::ClauseState;

fn spec(read: &[&str], write: &[&str]) -> ConfinementSpec {
    ConfinementSpec {
        read_paths: read.iter().map(PathBuf::from).collect(),
        write_paths: write.iter().map(PathBuf::from).collect(),
        deny_network: true,
        data_dir: PathBuf::from("/data"),
        ..ConfinementSpec::default()
    }
}

fn text(read: &[&str], write: &[&str]) -> String {
    generate(&spec(read, write)).expect("a valid specification")
}

fn at(profile: &str, needle: &str) -> usize {
    profile
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` is not in the profile:\n{profile}"))
}

/// RFC 011 Amendment 2 point 3: the profile starts from `(deny default)`
/// and nothing allows everything.
#[test]
fn rfc_011_a2_profile_denies_by_default() {
    let p = text(&["/usr", "/bin"], &["/data/workspaces"]);
    let lines: Vec<&str> = p
        .lines()
        .filter(|l| !l.starts_with(";;") && !l.is_empty())
        .collect();
    assert_eq!(lines[0], "(version 1)");
    assert_eq!(lines[1], "(deny default)");
    assert!(!p.contains("(allow default)"));
    // Every rule that grants files, execution or sockets names what it grants.
    for line in p.lines().filter(|l| l.starts_with("(allow ")) {
        let unqualified_ok = [
            "(allow process-fork)",
            "(allow file-read-metadata)",
            "(allow sysctl-read)",
        ];
        if unqualified_ok.contains(&line) {
            continue;
        }
        assert!(
            line.contains("(subpath ")
                || line.contains("(literal ")
                || line.contains("(target same-sandbox)"),
            "a rule without a filter would allow everything: {line}"
        );
    }
}

/// Amendment 2 point 3: an `allow` with no filter allows everything, so an
/// empty set writes no rule at all.
#[test]
fn rfc_011_a2_empty_sets_write_no_rule() {
    let p = text(&[], &[]);
    assert!(!p.contains("(allow file-read*)"));
    assert!(!p.contains("(allow file-write*"));
    assert!(!p.contains("(allow process-exec*"));
    // The one Unix-socket rule left is the explicit denial of the name-lookup socket.
    for line in p.lines().filter(|l| l.contains("unix-socket")) {
        assert!(line.starts_with("(deny "), "{line}");
    }
    assert!(!p.contains("(allow network"));
}

/// Amendment 2 point 3: the read set is readable and executable, the write
/// set readable, writable and executable (a build runs what it built).
#[test]
fn rfc_011_a2_sets_become_rules() {
    let p = text(&["/usr", "/opt/tool"], &["/data/workspaces", "/dev/null"]);
    let read = p
        .lines()
        .find(|l| l.starts_with("(allow file-read* (subpath \"/opt/tool\")"))
        .expect("a read rule");
    assert!(read.contains("(subpath \"/usr\")"));
    assert!(
        !read.contains("workspaces"),
        "the write set has its own rule"
    );
    let write = p
        .lines()
        .find(|l| l.starts_with("(allow file-write*"))
        .expect("a write rule");
    assert!(
        write.contains("(subpath \"/data/workspaces\")")
            && write.contains("(subpath \"/dev/null\")")
    );
    assert!(!write.contains("/usr"));
    let exec = p
        .lines()
        .find(|l| l.starts_with("(allow process-exec*"))
        .expect("an exec rule");
    for path in ["/usr", "/opt/tool", "/data/workspaces", "/dev/null"] {
        assert!(exec.contains(&format!("(subpath \"{path}\")")), "{path}");
    }
}

/// Amendment 2 point 3: SBPL string literals escape `\` and `"`, and a path
/// cannot end its literal and add a rule of its own (a hostile path,
/// REQ-SEC-013).
#[test]
fn rfc_011_a2_strings_are_escaped() {
    assert_eq!(escape(r#"a"b\c"#), r#"a\"b\\c"#);
    assert_eq!(escape("plain"), "plain");
    let hostile = r#"/x") (allow default) ("/y"#;
    let p = text(&[hostile], &[]);
    // The text is inside the literal, its quotes escaped; no rule of its own.
    assert!(p.lines().all(|l| !l.starts_with("(allow default")), "{p}");
    assert!(
        p.contains(r#"(subpath "/x\") (allow default) (\"/y")"#),
        "{p}"
    );
    let slashes = r"/a\b";
    let p = text(&[slashes], &[]);
    assert!(p.contains(r#"(subpath "/a\\b")"#), "{p}");
}

/// A path with a space (the default data directory is `~/Library/Application
/// Support/mimlys`) stays one literal.
#[test]
fn rfc_011_a2_spaces_stay_inside_the_literal() {
    let p = text(
        &[],
        &["/Users/a/Library/Application Support/mimlys/workspaces"],
    );
    assert!(
        p.contains("(subpath \"/Users/a/Library/Application Support/mimlys/workspaces\")"),
        "{p}"
    );
}

/// The generator refuses what it cannot write exactly, rather than guess.
#[test]
fn rfc_011_a2_unwritable_paths_are_refused() {
    let refused = |read: &[&str]| generate(&spec(read, &[])).unwrap_err();
    assert!(matches!(
        refused(&["relative/dir"]),
        ProfileError::NotAbsolute(_)
    ));
    assert!(matches!(
        refused(&["/a\nb"]),
        ProfileError::ControlCharacter(_)
    ));
    assert!(matches!(
        refused(&["/a\0b"]),
        ProfileError::ControlCharacter(_)
    ));
    assert_eq!(refused(&["/"]), ProfileError::Root);
    assert_eq!(refused(&["//"]), ProfileError::Root);
    assert!(matches!(
        generate(&spec(&[], &["x"])).unwrap_err(),
        ProfileError::NotAbsolute(_)
    ));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bad = ConfinementSpec {
            read_paths: vec![PathBuf::from(std::ffi::OsStr::from_bytes(b"/a\xffb"))],
            ..ConfinementSpec::default()
        };
        assert!(matches!(
            generate(&bad).unwrap_err(),
            ProfileError::NotUtf8(_)
        ));
    }
}

/// Amendment 2 point 3: paths have one spelling. A link is resolved, also
/// when the path does not exist yet (a workspace is created after the
/// profile is written. (Unix hosts: links are resolved by the file system.)
#[cfg(unix)]
#[test]
fn rfc_011_a2_paths_are_canonical() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().canonicalize().unwrap().join("real");
    std::fs::create_dir(&real).unwrap();
    {
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let s = ConfinementSpec {
            read_paths: vec![link.clone()],
            write_paths: vec![link.join("not/yet/there")],
            deny_network: true,
            ..ConfinementSpec::default()
        };
        let p = profile(&s).unwrap();
        assert!(
            p.contains(&format!("(subpath \"{}\")", real.display())),
            "{p}"
        );
        assert!(
            p.contains(&format!(
                "(subpath \"{}\")",
                real.join("not/yet/there").display()
            )),
            "{p}"
        );
        assert!(
            !p.contains(&format!("{}/link", dir.path().display())),
            "the link's own spelling is gone: {p}"
        );
        assert_eq!(canonical(&link.join("a/b")), real.join("a/b"));
    }
    assert_eq!(
        canonical(Path::new("/nonexistent-mimlys-dir/a")),
        Path::new("/nonexistent-mimlys-dir/a")
    );
    assert_eq!(canonical(Path::new("relative")), Path::new("relative"));
}

/// The profile is the same however the sets are ordered or repeated.
#[test]
fn rfc_011_a2_profile_is_deterministic() {
    let forward = spec(&["/usr", "/bin", "/sbin"], &["/data/b", "/data/a"]);
    let mut backward = forward.clone();
    backward.read_paths.reverse();
    backward.write_paths.reverse();
    backward.read_paths.push(PathBuf::from("/usr"));
    assert_eq!(generate(&forward).unwrap(), generate(&backward).unwrap());
}

/// Amendment 2 point 3: network is denied; Unix sockets only under the write
/// set, three rules each; no DNS: no mach service, no `mDNSResponder` socket.
#[test]
fn rfc_011_a2_network_is_denied_without_dns() {
    let p = text(&["/usr"], &["/data/workspaces", "/data/tmp"]);
    assert!(p.contains("(deny network*)"));
    assert!(!p.contains("(allow network*)"));
    for path in ["/data/workspaces", "/data/tmp"] {
        for rule in [
            format!("(allow network-bind (local unix-socket (subpath \"{path}\")))"),
            format!("(allow network-outbound (remote unix-socket (subpath \"{path}\")))"),
            format!("(allow network-inbound (local unix-socket (subpath \"{path}\")))"),
        ] {
            assert!(p.contains(&rule), "{rule}");
        }
    }
    for line in p.lines().filter(|l| l.starts_with("(allow network")) {
        assert!(
            line.contains("unix-socket (subpath \"/data/"),
            "only under the write set: {line}"
        );
    }
    assert_eq!(p.matches("(allow network").count(), 6);
    // DNS is a mach service and a Unix socket (measured): neither is named.
    for forbidden in ["mach", "dnssd", "(remote ip", "(remote tcp", "(remote udp"] {
        assert!(!p.contains(forbidden), "`{forbidden}` in\n{p}");
    }
    // The socket rules come after the denial: the last match wins.
    assert!(at(&p, "(allow network-bind") > at(&p, "(deny network*)"));
}

/// Amendment 2, settled, point 9: the offline profile names the
/// `mDNSResponder` socket in a `deny`, and that rule comes after every
/// Unix-socket allow (the last match wins), so that no write path nor a later
/// rule can reopen name resolution silently. It is never allowed.
#[test]
fn rfc_011_a2_offline_profile_denies_the_mdnsresponder_socket_last() {
    let p = text(&["/usr"], &["/data/workspaces", "/data/tmp"]);
    let deny =
        "(deny network-outbound (remote unix-socket (literal \"/private/var/run/mDNSResponder\")))";
    assert_eq!(p.matches(deny).count(), 1, "{p}");
    assert!(at(&p, deny) > p.rfind("(allow network").unwrap(), "{p}");
    assert!(!p.contains("(allow network-outbound (remote unix-socket (literal"));
    // A write set that holds the socket's directory is exactly the case it guards.
    let under_run = text(&["/usr"], &["/private/var/run"]);
    assert!(at(&under_run, deny) > under_run.rfind("(allow network").unwrap());
}

/// Amendment 2 point 9 is not decided: the dependency step's profile grants
/// no network rule beyond the same Unix sockets, and says where it waits.
#[test]
fn rfc_011_a2_dependency_step_profile_waits_for_point_9() {
    let mut s = spec(&["/usr"], &["/data/workspaces"]);
    s.deny_network = false;
    let p = generate(&s).unwrap();
    assert!(!p.contains("(deny network*)"));
    assert!(p.contains("point 9"));
    for line in p.lines().filter(|l| l.starts_with("(allow network")) {
        assert!(line.contains("unix-socket"), "{line}");
    }
    assert!(!p.contains("mach") && !p.contains("mDNSResponder"));
}

/// Amendment 2 point 4: other processes are invisible, and the order is
/// the one that makes the narrower rule win (the last match wins).
#[test]
fn rfc_011_a2_other_processes_are_invisible() {
    let p = text(&["/usr"], &["/data/workspaces"]);
    assert!(p.contains("(allow signal (target same-sandbox))"));
    assert!(!p.contains("(allow signal)"));
    assert!(at(&p, "(allow process-info* (target same-sandbox))") > at(&p, "(deny process-info*)"));
    assert!(
        !p.contains("(allow process-info*)"),
        "no listing of every process"
    );
    assert!(!p.contains("process-info-listpids"));
    // `kern.procargs*` is the one form that refuses another process's environment (S1).
    let deny = "(deny sysctl-read (sysctl-name-prefix \"kern.procargs\"))";
    assert!(at(&p, deny) > at(&p, "(allow sysctl-read)"));
    assert!(
        !p.contains("(sysctl-name \"kern.procargs"),
        "the `sysctl-name` form does not work"
    );
}

/// The loader reads the root directory; metadata is open as on Linux
/// (Landlock does not cover it); nothing else is read without a rule.
#[test]
fn rfc_011_a2_only_the_root_directory_is_read_outside_the_sets() {
    let p = text(&["/usr"], &["/data/workspaces"]);
    assert!(p.contains("(allow file-read* (literal \"/\"))"));
    assert!(p.contains("(allow file-read-metadata)\n"));
    for line in p.lines().filter(|l| l.starts_with("(allow file-read*")) {
        assert!(
            line.contains("(subpath ") || line == "(allow file-read* (literal \"/\"))",
            "{line}"
        );
    }
}

/// Amendment 2 point 8: the system part of the read set. `/opt` (Homebrew)
/// and the home directory are not in it; the developer directory is the
/// `xcode_select` link's target, else the Command Line Tools when present.
#[test]
fn rfc_011_a2_system_read_set() {
    let names = |paths: Vec<PathBuf>| -> Vec<String> {
        paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };
    let with_xcode = names(system_read_paths_from(
        Some(PathBuf::from(
            "/Applications/Xcode_26.6.app/Contents/Developer",
        )),
        true,
        false,
    ));
    for fixed in [
        "/usr",
        "/bin",
        "/sbin",
        "/System",
        "/private/etc",
        "/dev/urandom",
        "/dev/random",
        "/dev/zero",
    ] {
        assert!(with_xcode.iter().any(|p| p == fixed), "{fixed}");
    }
    assert!(with_xcode.contains(&"/Applications/Xcode_26.6.app/Contents/Developer".to_owned()));
    assert!(
        !with_xcode.contains(&COMMAND_LINE_TOOLS.to_owned()),
        "an Xcode is selected: the Command Line Tools are not added"
    );
    let tools = names(system_read_paths_from(None, true, false));
    assert!(tools.contains(&COMMAND_LINE_TOOLS.to_owned()));
    let relative = names(system_read_paths_from(
        Some(PathBuf::from("Developer")),
        false,
        false,
    ));
    assert!(
        !relative.iter().any(|p| p == "Developer"),
        "a relative link target is ignored"
    );
    let none = names(system_read_paths_from(None, false, false));
    assert_eq!(none.len(), 8);
    // Rosetta's runtime only where it is installed (settled point 8): a
    // translated tool aborts without it.
    assert!(!with_xcode.iter().any(|p| p == ROSETTA_RUNTIME));
    let rosetta = names(system_read_paths_from(None, false, true));
    assert_eq!(rosetta.len(), 9);
    assert!(rosetta.contains(&ROSETTA_RUNTIME.to_owned()));
    for never in [
        "/opt",
        "/Users",
        "/",
        "/private",
        "/private/var",
        "/Library",
    ] {
        assert!(!with_xcode.iter().any(|p| p == never), "{never}");
    }
}

/// The self-check looks only at directories the sets do not cover.
#[test]
fn rfc_011_a2_self_check_candidates_are_outside_the_sets() {
    let mut s = spec(
        &["/usr", "/Applications/Xcode.app/Contents/Developer"],
        &["/data/workspaces"],
    );
    let all = denied_candidates(&s);
    assert_eq!(
        all[0],
        PathBuf::from("/data"),
        "the runtime's own directory first"
    );
    assert!(
        all.contains(&PathBuf::from("/Applications")),
        "the parent of a read path is not itself readable"
    );
    s.read_paths.push(PathBuf::from("/Applications"));
    s.write_paths.push(PathBuf::from("/data"));
    let covered = denied_candidates(&s);
    assert!(!covered.contains(&PathBuf::from("/Applications")));
    assert!(!covered.contains(&PathBuf::from("/data")));
    assert!(covered.contains(&PathBuf::from("/Library")));
}

fn applied(filesystem: Option<&str>, network: Option<&str>) -> Outcome {
    Outcome::Applied {
        filesystem: filesystem.map(str::to_owned),
        network: network.map(str::to_owned),
    }
}

/// Amendment 2 points 2, 3 and 5: F, N and P are `enforced (seatbelt)` when
/// the profile was applied and the self-check held; K, M and C wait for
/// points 6 and 7 and are never reported enforced by this slice.
#[test]
fn rfc_011_a2_report_when_the_profile_holds() {
    let r = report(true, &applied(None, None));
    for letter in ["F", "N", "P"] {
        assert_eq!(r[letter], ClauseState::enforced("seatbelt"), "{letter}");
    }
    assert!(matches!(&r["K"], ClauseState::Unavailable { reason } if reason.contains("point 6")));
    for letter in ["M", "C"] {
        assert!(
            matches!(&r[letter], ClauseState::Unavailable { reason } if reason.contains("point 7")),
            "{letter}"
        );
    }
    assert_eq!(r.len(), 6, "E is the worker's own to report");
    // No outcome makes K, M or C better than unavailable.
    for outcome in [
        Outcome::Missing,
        Outcome::BadProfile("x".into()),
        Outcome::Rejected("x".into()),
        applied(None, None),
        applied(Some("x"), Some("y")),
    ] {
        for deny in [true, false] {
            let r = report(deny, &outcome);
            for letter in ["K", "M", "C"] {
                assert!(
                    matches!(r[letter], ClauseState::Unavailable { .. }),
                    "{letter} {outcome:?}"
                );
            }
        }
    }
}

/// Amendment 2 point 2: a removed `sandbox_init` is a reported state, and so
/// is a profile the system rejects (its message is kept) and a profile that
/// did not hold when the worker looked. Never `enforced` on faith.
#[test]
fn rfc_011_a2_report_when_the_profile_does_not_hold() {
    let r = report(true, &Outcome::Missing);
    for letter in ["F", "N", "P"] {
        assert_eq!(
            r[letter],
            ClauseState::unavailable("seatbelt_missing"),
            "{letter}"
        );
    }
    let r = report(
        true,
        &Outcome::Rejected(
            "unbound variable: defaultt at <input string>, line 1, column 18".into(),
        ),
    );
    for letter in ["F", "N", "P"] {
        assert!(
            matches!(&r[letter], ClauseState::Unavailable { reason }
                if reason.starts_with("seatbelt_rejected: ") && reason.contains("defaultt")),
            "{letter}: {:?}",
            r[letter]
        );
    }
    let r = report(true, &Outcome::BadProfile("path is not absolute: x".into()));
    assert!(
        matches!(&r["F"], ClauseState::Unavailable { reason } if reason.starts_with("seatbelt_profile: "))
    );
    // The file rule failed its check: F and P go, N keeps its own.
    let r = report(true, &applied(Some("/Users is readable"), None));
    assert!(
        matches!(&r["F"], ClauseState::Unavailable { reason } if reason.starts_with("seatbelt_ineffective: "))
    );
    assert!(
        matches!(&r["P"], ClauseState::Unavailable { reason } if reason.starts_with("seatbelt_ineffective: "))
    );
    assert!(r["N"].is_enforced());
    // The network rule failed its check: N goes alone.
    let r = report(
        true,
        &applied(None, Some("a loopback connection succeeded")),
    );
    assert!(
        matches!(&r["N"], ClauseState::Unavailable { reason } if reason.starts_with("seatbelt_ineffective: "))
    );
    assert!(r["F"].is_enforced() && r["P"].is_enforced());
}

/// The dependency step's worker holds F and P and reports N honestly
/// (Amendment 2 point 9 is not settled).
#[test]
fn rfc_011_a2_dependency_step_reports_n_unavailable() {
    let r = report(false, &applied(None, None));
    assert!(r["F"].is_enforced() && r["P"].is_enforced());
    assert!(matches!(&r["N"], ClauseState::Unavailable { reason } if reason.contains("point 9")));
}
