//! `xcrun`'s cache made by the runtime (REQ-CONF-005, RFC 011 Amendment 2,
//! implementation decisions, point 8; LIM-O8), with a stand-in for `xcrun`
//! (a script), on every Unix.

use super::*;

const XCODE: &str = "/Applications/Xcode_26.6.app/Contents/Developer";

/// [`ensure`] with the command the runtime gives on a native Mac.
#[cfg(unix)]
fn run(file: Option<PathBuf>, xcrun: &Path, wait: Duration) -> CacheState {
    let (program, args) = warming_command(xcrun, false);
    ensure(file, &program, &args, wait)
}

/// REQ-CONF-005: a translated runtime asks through `arch -arm64`; a native one directly.
#[test]
fn req_conf_005_a_translated_runtime_warms_through_arch_arm64() {
    let xcrun = Path::new("/usr/bin/xcrun");
    assert_eq!(
        warming_command(xcrun, false),
        (
            PathBuf::from("/usr/bin/xcrun"),
            vec!["--find".to_owned(), "clang".to_owned()]
        )
    );
    assert_eq!(
        warming_command(xcrun, true),
        (
            PathBuf::from("/usr/bin/arch"),
            vec![
                "-arm64".to_owned(),
                "/usr/bin/xcrun".to_owned(),
                "--find".to_owned(),
                "clang".to_owned()
            ]
        )
    );
}

#[test]
fn rfc_011_a2_cache_file_is_needed_by_an_xcode_only() {
    let temp = Path::new("/var/folders/h5/abc/T/");
    assert_eq!(
        cache_file(Some(Path::new(XCODE)), Some(temp)),
        Some(PathBuf::from("/var/folders/h5/abc/T/xcrun_db"))
    );
    // No trailing slash on the temporary directory, one on the developer directory.
    assert_eq!(
        cache_file(
            Some(Path::new("/Applications/Xcode.app/Contents/Developer/")),
            Some(Path::new("/var/folders/h5/abc/T"))
        ),
        Some(PathBuf::from("/var/folders/h5/abc/T/xcrun_db"))
    );
    // The Command Line Tools work without it; so does anything that is not an Xcode.
    for other in [
        "/Library/Developer/CommandLineTools",
        "/Library/Developer/Custom",
        "/Contents/Developer",
        "Contents/Developer",
        "relative/X.app/Contents/Developer",
    ] {
        assert_eq!(
            cache_file(Some(Path::new(other)), Some(temp)),
            None,
            "{other}"
        );
    }
    assert_eq!(cache_file(None, Some(temp)), None);
    assert_eq!(cache_file(Some(Path::new(XCODE)), None), None);
    assert_eq!(
        cache_file(Some(Path::new(XCODE)), Some(Path::new("T"))),
        None,
        "a relative temporary directory is not believed"
    );
}

#[cfg(unix)]
mod with_a_stand_in {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// The one executable script, written once: a test that wrote its own
    /// while another thread forked would meet `ETXTBSY` (the forked child
    /// holds the write handle until it execs). It runs the file `body` beside
    /// the path it was called by.
    fn shared_script() -> &'static (tempfile::TempDir, PathBuf) {
        static SCRIPT: std::sync::OnceLock<(tempfile::TempDir, PathBuf)> =
            std::sync::OnceLock::new();
        SCRIPT.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("runner");
            std::fs::write(
                &path,
                "#!/bin/sh\nexec sh \"$(dirname \"$0\")/body\" \"$@\"\n",
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            (dir, path)
        })
    }

    /// A script playing `xcrun`: a symlink to the shared runner, and a `body` beside it.
    fn stand_in(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("xcrun");
        std::os::unix::fs::symlink(&shared_script().1, &path).unwrap();
        std::fs::write(dir.join("body"), format!("{body}\n")).unwrap();
        path
    }

    /// REQ-CONF-005: where the cache is absent and an Xcode is selected, the
    /// runtime runs `xcrun --find clang` and the file exists afterwards.
    #[test]
    fn req_conf_005_a_missing_cache_is_made_by_running_xcrun_find_clang() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("xcrun_db");
        let args = dir.path().join("args");
        let xcrun = stand_in(
            dir.path(),
            &format!("echo \"$@\" > {}\ntouch {}", args.display(), file.display()),
        );
        let state = run(Some(file.clone()), &xcrun, Duration::from_secs(10));
        assert_eq!(state, CacheState::Present);
        assert!(file.exists());
        assert_eq!(
            std::fs::read_to_string(args).unwrap().trim(),
            "--find clang"
        );
    }

    /// REQ-CONF-005: nothing is run where the cache is there, or not needed.
    #[test]
    fn req_conf_005_xcrun_is_not_run_when_the_cache_is_there_or_not_needed() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let xcrun = stand_in(dir.path(), &format!("touch {}", marker.display()));
        let file = dir.path().join("xcrun_db");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            run(Some(file), &xcrun, Duration::from_secs(10)),
            CacheState::Present
        );
        assert_eq!(
            run(None, &xcrun, Duration::from_secs(10)),
            CacheState::NotNeeded
        );
        assert!(!marker.exists(), "xcrun was not run");
    }

    /// REQ-CONF-005 (external design §13.3): a cache that cannot be made is
    /// reported as missing, with the file named, and nothing panics: an
    /// `xcrun` that fails, one that does not exist.
    #[test]
    fn req_conf_005_a_cache_that_cannot_be_made_is_reported_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("xcrun_db");
        let failing = stand_in(dir.path(), "exit 1");
        assert_eq!(
            run(Some(file.clone()), &failing, Duration::from_secs(10)),
            CacheState::Missing { file: file.clone() }
        );
        assert_eq!(
            run(
                Some(file.clone()),
                &dir.path().join("no-such-xcrun"),
                Duration::from_secs(10)
            ),
            CacheState::Missing { file }
        );
    }

    /// LIM-O8: the wait is bounded. An `xcrun` that hangs is killed and the
    /// start goes on, the cache reported missing.
    #[test]
    fn lim_o8_the_wait_is_bounded_and_a_hanging_xcrun_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("xcrun_db");
        let pid = dir.path().join("pid");
        let xcrun = stand_in(
            dir.path(),
            &format!("echo $$ > {}\nexec sleep 30", pid.display()),
        );
        let started = Instant::now();
        let state = run(Some(file.clone()), &xcrun, Duration::from_millis(400));
        assert_eq!(state, CacheState::Missing { file });
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "returned after {:?}",
            started.elapsed()
        );
        let pid = std::fs::read_to_string(pid).unwrap().trim().to_owned();
        let alive = std::process::Command::new("ps")
            .args(["-p", &pid])
            .output()
            .unwrap()
            .status
            .success();
        assert!(!alive, "the hanging xcrun {pid} was killed and reaped");
    }

    /// REQ-CONF-005: a partial cache is not a cache. A hanging `xcrun` that
    /// wrote part of the file before it was killed, and one that wrote it and
    /// failed, leave nothing behind: the file would be taken for the cache and
    /// the sandboxed `xcrun` cannot repair it (measured under Rosetta, where
    /// the first launch of an Intel `xcrun` outlasted LIM-O8).
    #[test]
    fn req_conf_005_a_partial_cache_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("xcrun_db");
        let hanging = stand_in(
            dir.path(),
            &format!("echo partial > {}\nexec sleep 30", file.display()),
        );
        assert_eq!(
            run(Some(file.clone()), &hanging, Duration::from_millis(400)),
            CacheState::Missing { file: file.clone() }
        );
        assert!(!file.exists(), "the partial file was removed");
        let other = tempfile::tempdir().unwrap();
        let failing = stand_in(
            other.path(),
            &format!("echo partial > {}\nexit 1", file.display()),
        );
        assert_eq!(
            run(Some(file.clone()), &failing, Duration::from_secs(10)),
            CacheState::Missing { file: file.clone() }
        );
        assert!(!file.exists(), "and so was the file of a failed xcrun");
    }

    /// REQ-CONF-002 / clause E: the runtime's environment holds
    /// `env:`-referenced secrets and `xcrun` needs none of it: the stand-in
    /// sees an empty environment with a fixed `PATH`.
    #[test]
    fn req_conf_005_xcrun_gets_no_inherited_environment() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("xcrun_db");
        let seen = dir.path().join("env");
        let xcrun = stand_in(
            dir.path(),
            &format!(
                "echo \"${{HOME:-unset}} $PATH\" > {}\ntouch {}",
                seen.display(),
                file.display()
            ),
        );
        // The test process has a HOME; the stand-in must not.
        assert!(std::env::var_os("HOME").is_some());
        run(Some(file), &xcrun, Duration::from_secs(10));
        assert_eq!(
            std::fs::read_to_string(seen).unwrap().trim(),
            "unset /usr/bin:/bin"
        );
    }
}
