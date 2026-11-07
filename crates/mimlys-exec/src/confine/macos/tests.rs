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
            // The pid listing has no restricted form (settled point 6).
            "(allow process-info-listpids)",
        ];
        if unqualified_ok.contains(&line) {
            continue;
        }
        assert!(
            line.contains("(subpath ")
                || line.contains("(literal ")
                || line.contains("(target same-sandbox)")
                || line.contains("(global-name "),
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
/// set, three rules each; no DNS: no DNS mach service, no `mDNSResponder` socket
/// (the one mach service is `xcrun`'s, see `rfc_011_a2_one_mach_service`).
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
    for forbidden in ["dnssd", "(remote ip", "(remote tcp", "(remote udp"] {
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

/// Amendment 2, settled, point 9: the dependency step's profile adds exactly
/// three rules to the offline one: outbound TCP to port 443, a connect to the
/// `mDNSResponder` socket and the system's certificate file readable; no mach
/// service, no other network rule, no `(deny network*)`... and no
/// denial of the socket it needs.
#[test]
fn rfc_011_a2_dependency_step_profile_adds_three_rules() {
    let mut s = spec(&["/usr"], &["/data/workspaces"]);
    s.deny_network = false;
    let fetch = generate(&s).unwrap();
    s.deny_network = true;
    let offline = generate(&s).unwrap();
    assert!(!fetch.contains("(deny network"), "{fetch}");
    assert!(!fetch.contains("dnssd"), "{fetch}");
    let added: Vec<&str> = fetch
        .lines()
        .filter(|l| !l.starts_with(";;") && !offline.lines().any(|o| o == *l))
        .collect();
    assert_eq!(
        added,
        vec![
            "(allow network-outbound (remote tcp \"*:443\"))",
            "(allow network-outbound (remote unix-socket (literal \"/private/var/run/mDNSResponder\")))",
            "(allow file-read-data (literal \"/private/etc/ssl/cert.pem\"))",
        ],
        "{fetch}"
    );
    // What the offline profile has and the dependency step lacks: the two denials.
    let removed: Vec<&str> = offline
        .lines()
        .filter(|l| !fetch.lines().any(|f| f == *l))
        .collect();
    assert_eq!(
        removed,
        vec![
            "(deny network*)",
            "(deny network-outbound (remote unix-socket (literal \"/private/var/run/mDNSResponder\")))",
        ]
    );
}

/// The offline profile has none of the dependency step's rules.
#[test]
fn rfc_011_a2_offline_profile_has_no_dependency_rules() {
    let p = text(&["/usr"], &["/data/workspaces"]);
    for forbidden in [":443", "cert.pem", "(allow network-outbound (remote tcp"] {
        assert!(!p.contains(forbidden), "`{forbidden}` in\n{p}");
    }
    assert!(!p.contains("(allow network-outbound (remote unix-socket (literal"));
}

/// Amendment 2 point 4: other processes are invisible, and the order is
/// the one that makes the narrower rule win (the last match wins).
#[test]
fn rfc_011_a2_other_processes_are_invisible() {
    let p = text(&["/usr"], &["/data/workspaces"]);
    // The listing has no restricted form (measured): it is allowed without a
    // target, after the narrowing, and the members are told apart by
    // `proc_pidinfo`. The process list WITH NAMES stays refused (point 4,
    // settled): `kern.proc.` after the broad `sysctl-read` allow.
    assert!(
        at(&p, "(allow process-info-listpids)") > at(&p, "(deny process-info* (with no-report))")
    );
    assert!(!p.contains("(allow process-info-listpids (target"));
    assert!(
        at(&p, "(deny sysctl-read (sysctl-name-prefix \"kern.proc.\"))")
            > at(&p, "(allow sysctl-read)")
    );
    // The refusals of the membership loop's probing are not logged: it asks
    // about every pid of the machine at every poll.
    assert!(p.contains("(deny process-info* (with no-report))"));
    assert!(p.contains("(allow signal (target same-sandbox))"));
    assert!(!p.contains("(allow signal)"));
    assert!(
        at(&p, "(allow process-info* (target same-sandbox))")
            > at(&p, "(deny process-info* (with no-report))")
    );
    assert!(
        !p.contains("(allow process-info*)"),
        "no listing of every process"
    );
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

/// Amendment 2, settled, point 8: the system part of the read set is the
/// measured minimum. Not `/System` or `/private/etc` as a whole, not `/opt`
/// (Homebrew's), not the home directory; the developer directory is the
/// `xcode_select` link's target (and, for an Xcode, its `SharedFrameworks`),
/// else the Command Line Tools when present; `xcrun`'s cache file by name in
/// the per-user temporary directory; Rosetta's runtime where installed.
#[test]
fn rfc_011_a2_system_read_set() {
    let names = |paths: Vec<PathBuf>| -> Vec<String> {
        paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };
    let temp = || Some(PathBuf::from("/var/folders/h5/abc/T/"));
    let with_xcode = names(system_read_paths_from(
        Some(PathBuf::from(
            "/Applications/Xcode_26.6.app/Contents/Developer",
        )),
        true,
        false,
        temp(),
    ));
    for fixed in [
        "/usr",
        "/bin",
        "/sbin",
        "/private/etc/ssl/openssl.cnf",
        "/System/Library/CoreServices/SystemVersion.plist",
        "/dev/urandom",
        "/dev/random",
        "/dev/zero",
    ] {
        assert!(with_xcode.iter().any(|p| p == fixed), "{fixed}");
    }
    assert!(with_xcode.contains(&"/Applications/Xcode_26.6.app/Contents/Developer".to_owned()));
    assert!(
        with_xcode.contains(&"/Applications/Xcode_26.6.app/Contents/SharedFrameworks".to_owned()),
        "an Xcode's SharedFrameworks: {with_xcode:?}"
    );
    assert!(
        !with_xcode.contains(&COMMAND_LINE_TOOLS.to_owned()),
        "an Xcode is selected: the Command Line Tools are not added"
    );
    assert!(
        with_xcode.contains(&"/var/folders/h5/abc/T/xcrun_db".to_owned()),
        "xcrun's cache file, by name: {with_xcode:?}"
    );
    assert!(
        !with_xcode.contains(&"/var/folders/h5/abc/T".to_owned())
            && !with_xcode.contains(&"/var/folders/h5/abc/T/".to_owned()),
        "the temporary directory as a whole holds other programs' files"
    );
    let tools = names(system_read_paths_from(None, true, false, None));
    assert!(tools.contains(&COMMAND_LINE_TOOLS.to_owned()));
    assert!(
        !tools.iter().any(|p| p.ends_with("SharedFrameworks")),
        "only an Xcode has them"
    );
    let other_shape = names(system_read_paths_from(
        Some(PathBuf::from("/Library/Developer/Custom")),
        false,
        false,
        None,
    ));
    assert!(!other_shape.iter().any(|p| p.ends_with("SharedFrameworks")));
    let relative = names(system_read_paths_from(
        Some(PathBuf::from("Developer")),
        false,
        false,
        None,
    ));
    assert!(
        !relative.iter().any(|p| p == "Developer"),
        "a relative link target is ignored"
    );
    let relative_temp = names(system_read_paths_from(
        None,
        false,
        false,
        Some(PathBuf::from("T")),
    ));
    assert!(!relative_temp.iter().any(|p| p.ends_with("xcrun_db")));
    let none = names(system_read_paths_from(None, false, false, None));
    assert_eq!(none.len(), 8);
    // Rosetta's runtime only where it is installed: a translated tool aborts without it.
    assert!(!with_xcode.iter().any(|p| p == ROSETTA_RUNTIME));
    let rosetta = names(system_read_paths_from(None, false, true, None));
    assert_eq!(rosetta.len(), 9);
    assert!(rosetta.contains(&ROSETTA_RUNTIME.to_owned()));
    for never in [
        "/opt",
        "/Users",
        "/",
        "/private",
        "/private/etc",
        "/private/var",
        "/System",
        "/Library",
    ] {
        assert!(!with_xcode.iter().any(|p| p == never), "{never}");
    }
}

/// Amendment 2, settled, point 8: exactly one mach service is allowed, the
/// one `xcrun` asks for; the other interchangeable one, which can consult a
/// network directory, and every service that reaches the network or the
/// user's secrets, is not.
#[test]
fn rfc_011_a2_one_mach_service() {
    for deny_network in [true, false] {
        let mut s = spec(&["/usr"], &["/data/workspaces"]);
        s.deny_network = deny_network;
        let p = generate(&s).unwrap();
        let rules: Vec<&str> = p.lines().filter(|l| l.contains("mach-lookup")).collect();
        assert_eq!(
            rules,
            vec!["(allow mach-lookup (global-name \"com.apple.bsd.dirhelper\"))"],
            "{p}"
        );
        for refused in [
            "opendirectoryd",
            "trustd",
            "nsurlsessiond",
            "securityd",
            "dnssd",
            "mDNSResponder\"))",
        ] {
            assert!(
                !rules.iter().any(|r| r.contains(refused)),
                "`{refused}` in {rules:?}"
            );
        }
    }
    assert_eq!(MACH_SERVICES, ["com.apple.bsd.dirhelper"]);
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
        membership: None,
    }
}

fn applied_without_membership(why: &str) -> Outcome {
    Outcome::Applied {
        filesystem: None,
        network: None,
        membership: Some(why.to_owned()),
    }
}

/// Amendment 2 points 2 to 7: F, N and P are `enforced (seatbelt)` when the
/// profile was applied and the self-check held; K is `enforced
/// (seatbelt_membership)` and M and C are the `best_effort` watchdogs when the
/// worker verified the membership mechanism.
#[test]
fn rfc_011_a2_report_when_the_profile_holds() {
    let r = report(true, &applied(None, None));
    for letter in ["F", "N", "P"] {
        assert_eq!(r[letter], ClauseState::enforced("seatbelt"), "{letter}");
    }
    assert_eq!(r["K"], ClauseState::enforced("seatbelt_membership"));
    assert_eq!(r["M"], ClauseState::best_effort("rss_watchdog"));
    assert_eq!(r["C"], ClauseState::best_effort("process_count_watchdog"));
    assert_eq!(r.len(), 6, "E is the worker's own to report");
}

/// K, M and C are never better than unavailable unless the profile was
/// applied and the membership mechanism verified: no outcome reports them on
/// faith.
#[test]
fn rfc_011_a2_k_m_c_need_a_verified_membership() {
    for outcome in [
        Outcome::Missing,
        Outcome::BadProfile("x".into()),
        Outcome::Rejected("x".into()),
        applied_without_membership("the pid listing is refused"),
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
    let r = report(
        true,
        &applied_without_membership("the pid listing is refused"),
    );
    assert!(matches!(&r["K"], ClauseState::Unavailable { reason }
            if reason == "membership_unverified: the pid listing is refused"));
    // F, N and P are the profile's own and stay.
    assert!(r["F"].is_enforced() && r["N"].is_enforced() && r["P"].is_enforced());
    // A profile whose file check failed still reports K from its own check.
    let r = report(true, &applied(Some("/Users is readable"), None));
    assert!(r["K"].is_enforced());
}

/// Amendment 2 point 6: the worker verifies the restriction before relying
/// on it; each fact that fails names itself.
#[test]
fn rfc_011_a2_membership_check() {
    let good = MembershipFacts {
        listing_works: true,
        sees_itself: true,
        outsider_reachable: false,
        outsider_control: true,
    };
    assert_eq!(membership_check(&good), None);
    let cases = [
        (
            MembershipFacts {
                listing_works: false,
                ..good
            },
            "listing",
        ),
        (
            MembershipFacts {
                sees_itself: false,
                ..good
            },
            "itself",
        ),
        (
            MembershipFacts {
                outsider_control: false,
                ..good
            },
            "to check against",
        ),
        (
            MembershipFacts {
                outsider_reachable: true,
                ..good
            },
            "outside the sandbox",
        ),
    ];
    for (facts, word) in cases {
        let why = membership_check(&facts).unwrap_or_else(|| panic!("{facts:?} must fail"));
        assert!(why.contains(word), "{why}");
    }
    assert!(membership_check(&MembershipFacts::default()).is_some());
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

/// The dependency step's worker holds F, P and K and reports N honestly
/// (as on Linux): the network is allowed for it.
#[test]
fn rfc_011_a2_dependency_step_reports_n_unavailable() {
    let r = report(false, &applied(None, None));
    assert!(r["F"].is_enforced() && r["P"].is_enforced() && r["K"].is_enforced());
    assert_eq!(
        r["N"],
        ClauseState::unavailable("network allowed for the dependency step")
    );
}

fn member(pid: i32, resident: u64) -> Member {
    Member { pid, resident }
}

/// Amendment 2, settled, point 6: the members are the pids the profile lets
/// the worker inspect, live, other than the worker and `launchd`.
#[test]
fn rfc_011_a2_members_of_a_listing() {
    let listing = [0, 1, 50, 51, 52, 53, 54];
    let see = |pid: i32| match pid {
        0 | 1 => Seen::Alive { resident: 1 },
        50 => Seen::Alive { resident: 10 },
        51 => Seen::Hidden,
        52 => Seen::Zombie,
        53 => Seen::Alive { resident: 30 },
        _ => Seen::Hidden,
    };
    // 50 is the worker itself.
    assert_eq!(members_of(&listing, &[50], see), vec![member(53, 30)]);
    // With the worker elsewhere, 50 is a member; pid 0 and 1 are never.
    assert_eq!(
        members_of(&listing, &[99], see),
        vec![member(50, 10), member(53, 30)]
    );
    assert!(members_of(&[], &[99], see).is_empty());
    // The worker's sweeper is a member too and is not the tool's tree: the
    // worker excludes it with itself, and the sweeper (which excludes only
    // itself) kills everything else.
    assert_eq!(members_of(&listing, &[99, 50], see), vec![member(53, 30)]);
    assert_eq!(members_of(&listing, &[99, 53], see), vec![member(50, 10)]);
}

/// A fake process table: pids killed disappear after `lag` further listings
/// (a signal is not instant), and a storm adds a child to every member
/// listed until `births` children have been born.
struct Table {
    alive: Vec<i32>,
    dying: Vec<(i32, u32)>,
    lag: u32,
    births: u32,
    next: i32,
    killed: Vec<i32>,
}

impl Table {
    fn new(alive: &[i32], lag: u32, births: u32) -> Self {
        Self {
            alive: alive.to_vec(),
            dying: vec![],
            lag,
            births,
            next: 1000,
            killed: vec![],
        }
    }
    fn list(&mut self) -> Option<Vec<Member>> {
        self.dying
            .iter_mut()
            .for_each(|d| d.1 = d.1.saturating_sub(1));
        let gone: Vec<i32> = self
            .dying
            .iter()
            .filter(|d| d.1 == 0)
            .map(|d| d.0)
            .collect();
        self.alive.retain(|p| !gone.contains(p));
        self.dying.retain(|d| d.1 > 0);
        let listed: Vec<Member> = self.alive.iter().map(|&p| member(p, 1)).collect();
        if self.births > 0 && !listed.is_empty() {
            self.births -= 1;
            self.alive.push(self.next);
            self.next += 1;
        }
        Some(listed)
    }
    fn kill(&mut self, pid: i32) {
        self.killed.push(pid);
        if self.alive.contains(&pid) && !self.dying.iter().any(|d| d.0 == pid) {
            self.dying.push((pid, self.lag));
        }
    }
}

/// Amendment 2, settled, point 6: the kill loop repeats until a listing finds
/// no member: a signal that takes a moment and a storm that keeps forking
/// both end with an empty listing, and the report says so.
#[test]
fn rfc_011_a2_sweep_repeats_until_no_member_is_left() {
    use std::cell::RefCell;
    let table = RefCell::new(Table::new(&[10, 11, 12], 2, 5));
    let report = sweep_with(
        std::time::Duration::from_secs(5),
        || table.borrow_mut().list(),
        |pid| table.borrow_mut().kill(pid),
    );
    assert!(report.complete, "{report:?}");
    assert!(
        report.rounds >= 2,
        "a lagging signal needs another round: {report:?}"
    );
    assert!(table.borrow().alive.is_empty());
    for original in [10, 11, 12] {
        assert!(table.borrow().killed.contains(&original));
    }
    // The children born during the loop were found and killed too.
    assert!(table.borrow().killed.iter().any(|p| *p >= 1000));
    // Nothing to do: no signal, complete at once.
    let empty = RefCell::new(Table::new(&[], 0, 0));
    let r = sweep_with(
        std::time::Duration::from_secs(1),
        || empty.borrow_mut().list(),
        |pid| empty.borrow_mut().kill(pid),
    );
    assert_eq!(
        r,
        SweepReport {
            killed: 0,
            rounds: 0,
            complete: true
        }
    );
}

/// Amendment 2, settled, point 6: a member that never dies is reported
/// incomplete after LIM-O2 (`tree_kill_incomplete`), and a failing listing
/// is incomplete at once: never "complete" on faith.
#[test]
fn rfc_011_a2_sweep_reports_what_it_could_not_do() {
    let immortal = sweep_with(
        std::time::Duration::from_millis(60),
        || Some(vec![member(10, 1)]),
        |_| {},
    );
    assert!(!immortal.complete && immortal.rounds >= 1, "{immortal:?}");
    let blind = sweep_with(
        std::time::Duration::from_secs(1),
        || None,
        |_| panic!("no kill"),
    );
    assert_eq!(
        blind,
        SweepReport {
            killed: 0,
            rounds: 0,
            complete: false
        }
    );
}

/// Amendment 2, settled, point 7: M is the members' summed resident size
/// strictly above the ceiling, C their number strictly above the ceiling.
#[test]
fn rfc_011_a2_watchdog_verdicts() {
    let gib = 1024 * 1024 * 1024u64;
    let two = [member(1, gib), member(2, gib)];
    assert_eq!(
        verdict(&two, 2 * gib, 64),
        None,
        "at the ceiling is not above"
    );
    assert_eq!(
        verdict(&two, 2 * gib - 1, 64),
        Some(Verdict::Memory { resident: 2 * gib })
    );
    let many: Vec<Member> = (0..65).map(|p| member(p + 2, 1)).collect();
    assert_eq!(verdict(&many[..64], 2 * gib, 64), None);
    assert_eq!(
        verdict(&many, 2 * gib, 64),
        Some(Verdict::Count { count: 65 })
    );
    // Memory is judged first.
    let both: Vec<Member> = (0..65).map(|p| member(p + 2, gib)).collect();
    assert!(matches!(
        verdict(&both, 2 * gib, 64),
        Some(Verdict::Memory { .. })
    ));
    // No members, no verdict; a ceiling of 0 watches nothing for it.
    assert_eq!(verdict(&[], 2 * gib, 64), None);
    assert_eq!(verdict(&many, 0, 0), None);
}
