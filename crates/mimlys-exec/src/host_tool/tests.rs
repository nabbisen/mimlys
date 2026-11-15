//! Linux and macOS: Windows keeps its own `PATH` rule (external design §13.3).
#![cfg(unix)]

use super::*;
use std::collections::BTreeMap;

fn declaration(command: &str, read_paths: &[&Path]) -> HostToolDeclaration {
    HostToolDeclaration {
        name: "tool".into(),
        command: command.into(),
        args: vec![],
        allowed_args: vec![],
        arg_pattern: None,
        max_args: 0,
        effect: mimlys_core::action::EffectKind::WorkspaceWrite,
        reversible: true,
        timeout_ms: 10_000,
        read_paths: read_paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        env: BTreeMap::new(),
        env_allow: vec![],
        network: false,
        allow_paths: false,
    }
}

fn path_of(env: &[(String, String)]) -> String {
    env.iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .expect("PATH is always set")
}

/// REQ-CONF-006 / external design §13.3, §13.5: the tool's `PATH` is built
/// from the declared read paths that are themselves a `bin` directory
/// (`~/.cargo/bin`) or hold one, then `/usr/bin:/bin`; a declaration's `env`
/// that sets `PATH` wins; a command given as a name resolves against the
/// same directories. (Linux and macOS: Windows keeps its own rule.)
#[test]
fn req_conf_006_path_is_built_from_declared_bin_paths() {
    let root = tempfile::tempdir().unwrap();
    // A declared `…/bin` itself, a declared parent that holds `bin`, and a
    // directory that is neither (a toolchain home with no `bin`).
    let cargo_bin = root.path().join("cargo/bin");
    let parent = root.path().join("sdk");
    let neither = root.path().join("rustup");
    std::fs::create_dir_all(&cargo_bin).unwrap();
    std::fs::create_dir_all(parent.join("bin")).unwrap();
    std::fs::create_dir_all(neither.join("toolchains")).unwrap();
    let tmp = root.path().join("tmp");

    let d = declaration("tool", &[&cargo_bin, &parent, &neither]);
    let expected = format!(
        "{}:{}:/usr/bin:/bin",
        cargo_bin.display(),
        parent.join("bin").display()
    );
    assert_eq!(path_of(&env_for(&d, &tmp)), expected);

    // Each case alone.
    assert_eq!(
        path_of(&env_for(&declaration("tool", &[&cargo_bin]), &tmp)),
        format!("{}:/usr/bin:/bin", cargo_bin.display()),
        "a declared bin directory is itself on PATH, not `bin/bin`"
    );
    assert_eq!(
        path_of(&env_for(&declaration("tool", &[&parent]), &tmp)),
        format!("{}:/usr/bin:/bin", parent.join("bin").display()),
        "a declared parent contributes its `bin`"
    );
    assert_eq!(
        path_of(&env_for(&declaration("tool", &[&neither]), &tmp)),
        "/usr/bin:/bin",
        "a directory that is neither adds nothing"
    );

    // The declaration's own PATH wins over the built one.
    let mut own = declaration("tool", &[&cargo_bin]);
    own.env.insert("PATH".into(), "/custom/bin:/usr/bin".into());
    let env = env_for(&own, &tmp);
    assert_eq!(path_of(&env), "/custom/bin:/usr/bin");
    assert_eq!(
        env.iter().filter(|(k, _)| k == "PATH").count(),
        1,
        "set once"
    );

    // A command given as a name resolves against the same directories.
    std::fs::write(cargo_bin.join("cargo"), "#!/bin/sh\n").unwrap();
    std::fs::write(parent.join("bin/helper"), "#!/bin/sh\n").unwrap();
    let both = declaration("cargo", &[&cargo_bin, &parent, &neither]);
    assert_eq!(resolve_command(&both).unwrap(), cargo_bin.join("cargo"));
    let helper = declaration("helper", &[&cargo_bin, &parent, &neither]);
    assert_eq!(resolve_command(&helper).unwrap(), parent.join("bin/helper"));
    let missing = declaration("absent", &[&cargo_bin, &parent, &neither]);
    assert!(
        resolve_command(&missing)
            .unwrap_err()
            .contains("no declared bin directory")
    );
}

/// A shell that prints its own address-space limit in KiB, as a host tool.
#[cfg(target_os = "linux")]
fn limit_reporter() -> HostToolDeclaration {
    let mut d = declaration("/bin/sh", &[]);
    d.args = vec!["-c".into(), "ulimit -v".into()];
    d
}

/// REQ-CONF-005, REQ-SEC-011 (handoff 011n item 7), Linux: clause M on the
/// `rlimit_as` rung is set in the child, between fork and exec, and never in
/// the worker. Under many spawns, with the limit far below this process's own
/// address space, (a) another thread's allocations never fail, (b) this
/// process's limit is what it was after each spawn, and (c) the tool does
/// have the limit (the shell reports it): the limit still applies to the tool.
#[cfg(target_os = "linux")]
#[test]
fn req_conf_005_the_address_space_limit_is_the_childs_not_the_workers() {
    use rustix::process::{Resource, getrlimit};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    // Run in a process of its own (this binary again, with this one test): the
    // forks of forty spawns must not meet the other tests of the binary, which
    // write and execute files of their own (a fork holding a file open that
    // another thread then executes fails with "text file busy").
    const CHILD: &str = "MIMLYS_RLIMIT_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .args([
                "--exact",
                "host_tool::tests::req_conf_005_the_address_space_limit_is_the_childs_not_the_workers",
                "--test-threads=1",
                "--nocapture",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "the test, run alone, failed");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let sandbox = crate::sandbox::Sandbox::new().unwrap();
    let d = limit_reporter();
    let before = getrlimit(Resource::As);
    // 64 MiB: below the address space of this process, so that a limit lowered
    // here would make a thread's next large allocation fail.
    let limit: u64 = 64 << 20;
    let stop = AtomicBool::new(false);
    let failures = AtomicU64::new(0);
    let attempts = AtomicU64::new(0);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                let mut v: Vec<u8> = Vec::new();
                attempts.fetch_add(1, Ordering::Relaxed);
                if v.try_reserve_exact(8 << 20).is_err() {
                    failures.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::yield_now();
            }
        });
        for round in 0..40 {
            let interrupt = sandbox.interrupt(10_000);
            let outcome = run(HostRun {
                declaration: &d,
                extra_args: vec![],
                workspace: root.path(),
                tmp: root.path(),
                deadline_ms: 10_000,
                interrupt: &interrupt,
                rlimit_memory: Some(limit),
                tool_cgroup: None,
                tree_limits: TreeLimits::default(),
            });
            let said = String::from_utf8_lossy(&outcome.stdout).trim().to_owned();
            assert_eq!(
                said,
                (limit / 1024).to_string(),
                "round {round}: the tool's limit ({:?})",
                outcome.termination
            );
            assert_eq!(
                getrlimit(Resource::As),
                before,
                "round {round}: this process's limit is untouched"
            );
        }
        stop.store(true, Ordering::Relaxed);
    });
    assert!(attempts.load(Ordering::Relaxed) > 0, "the allocator ran");
    assert_eq!(
        failures.load(Ordering::Relaxed),
        0,
        "another thread's allocation failed during a spawn ({} attempts)",
        attempts.load(Ordering::Relaxed)
    );
}

/// The rule itself, without a filesystem: only a directory that is named
/// `bin`, or that the test says holds one, contributes.
#[test]
fn req_conf_006_bin_dirs_rule_without_a_filesystem() {
    let held = Path::new("/h/sdk/bin");
    let dirs = bin_dirs_with(
        &["/h/.cargo/bin".into(), "/h/sdk".into(), "/h/.rustup".into()],
        |d| d == held,
    );
    assert_eq!(
        dirs,
        vec![PathBuf::from("/h/.cargo/bin"), PathBuf::from("/h/sdk/bin")]
    );
}
