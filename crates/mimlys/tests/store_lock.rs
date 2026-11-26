//! Every command that writes names the holder when the store's lock refuses it (LIM-O3,
//! REQ-PERS-013; handoff 015a-5, item 6), through the binary: `model probe` was the only one that
//! did, and the rest said `store locked by another runtime`. The holder is another process of the
//! user's; the test holds the store itself, as a runtime does, so the holder is this process.

use serde_json::Value;
use std::path::PathBuf;

mod common;
use common::run_bounded;

struct Rig {
    registry: common::Registry,
    dir: tempfile::TempDir,
    config: PathBuf,
    scope: PathBuf,
}

impl Rig {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let scope = dir.path().join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::write(scope.join("a.txt"), "a\n").unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 2\n[runtime]\ndata_dir = {:?}\n[backends.default]\nkind = \"fake\"\n",
                dir.path().join("data").to_string_lossy()
            ),
        )
        .unwrap();
        Self {
            registry: common::Registry::new(),
            dir,
            config,
            scope,
        }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "");
        let out = run_bounded(&mut cmd, self.dir.path());
        (out.code(), out.stdout.clone(), out.stderr.clone())
    }
}

/// LIM-O3: while another process holds the store, each command that writes is refused with the
/// holder named (its process and since when), in text, and in JSON as the error kind `conflict`
/// with `detail.holder_pid`; none of them says only that the store is locked. A command that only
/// reads is not refused (it opens the store as a reader).
#[test]
fn lim_o3_every_writer_names_the_holder() {
    let rig = Rig::new();
    let scope = rig.scope.to_str().unwrap().to_owned();
    let (code, created, stderr) =
        rig.run(&["--json", "task", "new", "go", "--scope", &scope, "--hold"]);
    assert_eq!(code, 0, "{stderr}");
    let created: Value = serde_json::from_str(created.trim()).unwrap();
    let id = created["task"]["task_id"].as_str().unwrap().to_owned();
    let grant = "01a1eecb-0000-7000-8000-000000000000";

    let holder = mimlys_persist::open(&rig.data()).expect("this test holds the store");
    let pid = std::process::id();
    let commands: Vec<Vec<&str>> = vec![
        vec!["run", "go", "--scope", &scope],
        vec!["task", "new", "go", "--scope", &scope, "--hold"],
        vec!["task", "resume", &id],
        vec!["task", "pause", &id],
        vec!["task", "cancel", &id],
        vec!["task", "delete", &id, "--yes"],
        vec!["task", "prune", "--yes"],
        vec!["approve", &id],
        vec!["reject", &id],
        vec!["answer", &id, "yes"],
        vec!["grant", &id, "tool", "read_file"],
        vec!["revoke", &id, grant],
        vec!["rollback", &id],
        vec!["promote", &id, "--yes"],
        vec!["interpreters", "list"],
        vec!["model", "probe"],
    ];
    for args in &commands {
        let (code, stdout, stderr) = rig.run(args);
        assert_eq!(code, 1, "{args:?}: {stdout}\n{stderr}");
        assert!(
            stderr.contains(&format!("the store is in use by runtime {pid} since")),
            "{args:?}: the holder is not named: {stderr}"
        );
        // Each refusal waits a second for the lock: the JSON form is read for three of them.
        if !["run", "grant", "model"].contains(&args[0]) {
            continue;
        }
        let mut json_args = vec!["--json"];
        json_args.extend(args.iter().copied());
        let (code, stdout, stderr) = rig.run(&json_args);
        assert_eq!(code, 1, "{args:?}: {stdout}\n{stderr}");
        let printed: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("{args:?}: not JSON ({e}): {stdout}\n{stderr}"));
        assert_eq!(printed["error"]["kind"], "conflict", "{args:?}: {printed}");
        assert_eq!(
            printed["error"]["detail"]["holder_pid"], pid,
            "{args:?}: {printed}"
        );
    }
    // A reader is not refused, and says who holds the store.
    let (code, stdout, stderr) = rig.run(&["status"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains(&format!("store: in use by runtime {pid} since")),
        "{stdout}"
    );
    drop(holder);
    // Released, the same command is not refused.
    let (code, _, stderr) = rig.run(&["task", "pause", &id]);
    assert!(!stderr.contains("in use by runtime"), "{stderr}");
    let _ = code;
}
