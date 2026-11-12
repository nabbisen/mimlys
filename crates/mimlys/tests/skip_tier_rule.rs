//! The rule that asserts the Linux full tier (handoff 002d item 11): the CI
//! step runs `scripts/check-skip-tier.sh` over `skips.log`. This runs the
//! script over crafted logs: a skip whose reason is the tier fails it and is
//! named; the skips that are allowed by name, and skips for another cause, do not.
//! Unix only: the script is POSIX `sh`, and only the Linux jobs run it.

#![cfg(unix)]

mod common;

use std::path::PathBuf;
use std::process::Command;

fn script() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-skip-tier.sh");
    path.is_file().then_some(path)
}

/// Runs the script over a `skips.log` with `lines`; returns (exit code, stderr).
fn run(script: &PathBuf, lines: &[&str]) -> (i32, String) {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("skips.log");
    let mut text = lines.join("\n");
    if !lines.is_empty() {
        text.push('\n');
    }
    std::fs::write(&log, text).unwrap();
    let out = Command::new("sh").arg(script).arg(&log).output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// REQ-PLAT-003: the Linux full tier is asserted. A skip whose reason is the
/// tier fails the check and is named; the demonstration's rustup skip and the
/// opt-in tests' are allowed by name; a test with "tier" in its name that skips
/// for another cause is allowed; a line the rule cannot read fails.
#[test]
fn req_plat_003_a_skip_on_the_tier_fails_the_linux_check() {
    let Some(script) = script() else {
        common::skip(
            "req_plat_003_a_skip_on_the_tier_fails_the_linux_check",
            "scripts/ is not in this source tree",
        );
        return;
    };
    // No skips, or an absent file, is a pass.
    assert_eq!(run(&script, &[]).0, 0);
    let missing = Command::new("sh")
        .arg(&script)
        .arg("/nonexistent/skips.log")
        .status()
        .unwrap();
    assert_eq!(missing.code(), Some(0));

    // The tier is the reason: fails, and the line is named.
    for line in [
        "SKIPPED m6a_clauses_hold_inside_a_host_tool: this machine is tier core ({\"network\": \"x\"})",
        "SKIPPED m6a_argument_rules_are_enforced: tier basic",
        "SKIPPED m6a_demonstration_cargo_test_offline: tier \"core\"",
    ] {
        let (code, stderr) = run(&script, &["SKIPPED other: ordinary", line]);
        assert_eq!(code, 1, "{line}");
        assert!(stderr.contains(line), "{line}: {stderr}");
        assert!(!stderr.contains("ordinary"), "{stderr}");
    }

    // Allowed by name, or not the tier's.
    let allowed = [
        "SKIPPED m6a_demonstration_cargo_test_offline: no rustup toolchain under /home/runner",
        "SKIPPED m5_real_rypsok: MIMLYS_RYPSOK_BIN unset",
        "SKIPPED m4_real_engine_repairs_the_defect: MIMLYS_M4_BACKEND_URL unset",
        "SKIPPED m6a_host_tool_approval_shows_the_tier: io_uring is not usable here: EPERM",
        "SKIPPED req_conf_012_landlock_tcp_layer_denies_connect_and_bind: no Landlock on this kernel",
        // A corpus case is named by its file: a name is not a reason.
        "SKIPPED paths/tier-names-a-case.json: creates symbolic links with std::os::unix",
        "SKIPPED req_ws_014_prune: this user can write a directory without the permission (root)",
    ];
    assert_eq!(run(&script, &allowed), (0, String::new()));

    // A line that is not "SKIPPED <test>: <reason>" is not taken for a pass.
    let (code, stderr) = run(&script, &["not a skip line"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("unreadable: not a skip line"), "{stderr}");
}
