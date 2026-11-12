//! A skipped unit test is counted (REQ-PLAT-003, handoff 002d item 12).
//!
//! Test code only (`#[cfg(test)]` in `lib.rs`): nothing here is in the shipped
//! library. The integration tests of the `mimlys` crate record a skip with
//! `common::skip` (`crates/mimlys/tests/common`), which appends
//! `SKIPPED <test>: <reason>` to `skips.log` in `CARGO_TARGET_TMPDIR`. A unit
//! test in `src/` cannot reach that module, and cargo sets
//! `CARGO_TARGET_TMPDIR` for integration tests only, so this helper writes the
//! same line to the same file and finds the directory another way (below). Keep
//! the line's format in step with `common::skip`; CI reads the file in two steps
//! (the count, and `scripts/check-skip-tier.sh`).

use std::path::{Path, PathBuf};

/// The directory of `skips.log`: where `CARGO_TARGET_TMPDIR` points for the
/// integration tests of the same build.
///
/// In order: `CARGO_TARGET_TMPDIR` when the environment has it; else
/// `$CARGO_TARGET_DIR/tmp`; else `<workspace root>/target/tmp`, the root being
/// the nearest ancestor of this crate's manifest directory that holds
/// `Cargo.lock`. The executable's own path is not used: since cargo 1.91 the
/// unit-test executables live under `target/<profile>/build/<crate>/<hash>/out/`.
/// A `--target <triple>` build puts the integration tests' directory at
/// `target/<triple>/tmp`; the one unit test that records a skip is Linux's, and
/// CI runs it without `--target`.
fn tmp_dir(manifest_dir: &Path) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_TMPDIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join("tmp"));
    }
    manifest_dir
        .ancestors()
        .find(|dir| dir.join("Cargo.lock").is_file())
        .map(|root| root.join("target").join("tmp"))
}

/// Appends `SKIPPED <test>: <reason>` to `skips.log` in `dir` (created if missing).
fn record(dir: &Path, test: &str, reason: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("skips.log"))?;
    // One write per line: appends from parallel tests do not interleave.
    file.write_all(format!("SKIPPED {test}: {reason}\n").as_bytes())
}

/// Records that a unit test did not run its subject, and why: printed to
/// standard error and appended to the `skips.log` that CI counts.
// Used by the Linux unit tests only (the Landlock one).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn skip(test: &str, reason: &str) {
    eprintln!("SKIPPED {test}: {reason}");
    if let Some(dir) = tmp_dir(Path::new(env!("CARGO_MANIFEST_DIR"))) {
        let _ = record(&dir, test, reason);
    }
}

#[test]
fn the_skip_log_is_the_one_ci_reads_and_has_the_format_of_common_skip() {
    // The workspace root is found from this crate's manifest directory, and
    // `<root>/target/tmp` is what the integration tests of a plain
    // `cargo test` get as CARGO_TARGET_TMPDIR (this process has none set).
    if std::env::var_os("CARGO_TARGET_TMPDIR").is_none()
        && std::env::var_os("CARGO_TARGET_DIR").is_none()
    {
        let dir = tmp_dir(Path::new(env!("CARGO_MANIFEST_DIR"))).expect("a workspace root");
        assert!(dir.ends_with("target/tmp"), "{dir:?}");
        assert!(
            dir.parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("Cargo.lock")
                .is_file()
        );
    }
    // And the lines are written as `common::skip` writes them.
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "a_test", "no Landlock on this kernel").unwrap();
    record(dir.path(), "b_test", "Landlock ABI below 4").unwrap();
    let text = std::fs::read_to_string(dir.path().join("skips.log")).unwrap();
    assert_eq!(
        text,
        "SKIPPED a_test: no Landlock on this kernel\nSKIPPED b_test: Landlock ABI below 4\n"
    );
}
