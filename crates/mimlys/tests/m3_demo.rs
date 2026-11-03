//! The M3 demonstration (external design §23; RFC 006): a failed write
//! execution leaves the workspace at its pre-execution snapshot; promotion
//! refuses a conflict and otherwise shows the diff, asks, backs up and
//! applies, and reverses; a terminal task is deleted with its footprint
//! shown; a fork starts from a snapshot with the history attached.
//!
//! The symbolic-link rule in both directions is `workspace::tests::snapshots::
//! req_ws_005_symlinks_both_directions` and `req_ws_002_symlink_rule`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::run_bounded;

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/m3")
        .join(name)
}

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("tools")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "fn a() -> i32 { 0 }\n").unwrap();
    std::fs::write(dir.path().join("src/old.rs"), "// to be deleted\n").unwrap();
    std::fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
    // Creates `/workspace/partial.txt`, writes into it, then exits 1 (a failed write-effect execution).
    let write_then_fail = r#"(module
        (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
        (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
        (memory 1) (export "memory" (memory 0))
        (data (i32.const 0) "partial.txt") (data (i32.const 16) "half-written")
        (func (export "_start")
            (drop (call $open (i32.const 3) (i32.const 0) (i32.const 0) (i32.const 11) (i32.const 1) (i64.const 0x40) (i64.const 0) (i32.const 0) (i32.const 100)))
            (i32.store (i32.const 200) (i32.const 16)) (i32.store (i32.const 204) (i32.const 12))
            (drop (call $fd_write (i32.load (i32.const 100)) (i32.const 200) (i32.const 1) (i32.const 300)))
            (call $exit (i32.const 1))))"#;
    std::fs::write(
        dir.path().join("tools/write_then_fail.wasm"),
        wat::parse_str(write_then_fail).unwrap(),
    )
    .unwrap();
    dir
}

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new(script_name: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"fake\"\nscript = {:?}\n",
                data.path().join("data").to_string_lossy(),
                script(script_name).to_string_lossy()
            ),
        )
        .unwrap();
        Self { data, config }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.data.path());
        (out.code(), out.stdout, out.stderr)
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }

    fn tree(&self, task_id: &str) -> PathBuf {
        self.data
            .path()
            .join("data/workspaces")
            .join(task_id)
            .join("tree")
    }
}

/// External design §23, M3: a failed write execution leaves the workspace
/// at its pre-execution snapshot; the observation says so; a successful
/// write before it persists.
#[test]
fn m3_failed_write_is_rolled_back() {
    let env = Env::new("failed_write.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let tree = env.tree(&id);
    assert_eq!(
        std::fs::read_to_string(tree.join("notes.txt")).unwrap(),
        "kept: a successful write persists\n"
    );
    assert!(
        !tree.join("partial.txt").exists(),
        "the failed execution's write was rolled back"
    );
    let (_, events) = env.json(&["events", &id]);
    let names: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["record"]["event"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"workspace.rollback"), "{names:?}");
    let snapshots: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["record"]["event"] == "workspace.snapshot")
        .map(|e| e["record"]["reason"].as_str().unwrap())
        .collect();
    // The first write's `before` reuses the base and the failed execution's `before` reuses the
    // previous `after`: identical manifests share a record (RFC 006 §2).
    assert_eq!(snapshots, ["base", "after"], "{snapshots:?}");
    assert_eq!(value["snapshots"].as_array().unwrap().len(), 2);
    assert!(value["footprint"]["snapshots"] == 2);
}

/// External design §23, M3: promotion refuses a conflicting file and
/// otherwise shows the diff, needs confirmation, backs up and applies; a
/// patch writes nothing; the promotion is reversed from its backup.
#[test]
fn m3_promotion() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (code, value) = env.json(&["run", "edit", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    // The patch writes nothing.
    let (code, patch, _) = env.run(&["promote", &id, "--patch"]);
    assert_eq!(code, 0);
    assert!(patch.contains("+fn a() -> i32 { 1 }"), "{patch}");
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    // Without a terminal and without --yes: the plan, nothing written.
    let (code, value) = env.json(&["promote", &id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["applied"], false);
    assert_eq!(value["plan"]["changes"].as_array().unwrap().len(), 3);
    assert_eq!(value["plan"]["excluded"], serde_json::json!([]));
    assert!(scope.path().join("src/old.rs").exists());
    // A conflict: the user edits a target.
    std::fs::write(scope.path().join("src/lib.rs"), "fn a() -> i32 { 42 }\n").unwrap();
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["kind"], "conflict");
    assert_eq!(
        value["error"]["detail"]["paths"],
        serde_json::json!(["src/lib.rs"])
    );
    std::fs::write(scope.path().join("src/lib.rs"), "fn a() -> i32 { 0 }\n").unwrap();
    // Apply.
    let (code, value) = env.json(&["promote", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["applied"], true);
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 1 }\n"
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/added.rs")).unwrap(),
        "pub fn added() {}\n"
    );
    assert!(!scope.path().join("src/old.rs").exists());
    assert_eq!(
        std::fs::read_to_string(scope.path().join(".env")).unwrap(),
        "SECRET=1"
    );
    let promotion_id = value["promotion"]["promotion_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, view) = env.json(&["task", "show", &id]);
    assert_eq!(view["promotions"].as_array().unwrap().len(), 1);
    assert_eq!(view["footprint"]["promotions"], 1);
    // Reverse.
    let (code, value) = env.json(&["rollback", &id, "--promotion", &promotion_id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
    assert!(!scope.path().join("src/added.rs").exists());
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/old.rs")).unwrap(),
        "// to be deleted\n"
    );
    let (_, view) = env.json(&["task", "show", &id]);
    assert!(view["promotions"][0]["reversed_at"].is_string());
    // The user's rollback of the workspace to its base.
    let base = view["snapshots"][0]["snapshot_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, _) = env.json(&["rollback", &id, "--snapshot", &base]);
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_to_string(env.tree(&id).join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 0 }\n"
    );
}

/// RFC 006 §6: deletion needs a terminal task, shows what goes, keeps the audit unless asked.
#[test]
fn m3_delete_with_footprint() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (_, value) = env.json(&[
        "task",
        "new",
        "edit",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["task", "delete", &id, "--yes"]);
    assert_eq!(code, 1, "not before the task is terminal");
    assert_eq!(value["error"]["kind"], "state");
    env.json(&["task", "cancel", &id]);
    let (code, value) = env.json(&["task", "delete", &id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        value["deleted"], false,
        "without --yes and without a terminal nothing goes"
    );
    assert!(env.tree(&id).exists());
    let (_, list) = env.json(&["task", "list"]);
    assert!(list[0]["footprint"]["workspace_bytes"].as_u64().unwrap() > 0);
    let (code, value) = env.json(&["task", "delete", &id, "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["deleted"], true);
    assert!(!env.tree(&id).exists());
    assert!(
        std::fs::read_dir(env.data.path().join("data/snapshots/manifests"))
            .unwrap()
            .next()
            .is_none(),
        "no manifest left"
    );
    assert!(
        std::fs::read_dir(env.data.path().join("data/snapshots/objects"))
            .unwrap()
            .next()
            .is_none(),
        "no object left"
    );
    let (_, events) = env.json(&["events", &id]);
    let names: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["record"]["event"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"task.deleted"),
        "the audit stays: {names:?}"
    );
    let (code, _) = env.json(&["task", "delete", &id, "--yes", "--audit"]);
    assert_eq!(code, 0);
    let (code, value) = env.json(&["task", "show", &id]);
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["error"]["kind"], "not_found");
}

/// REQ-TASK-004: a fork from the source's latest snapshot, with its grants and history.
#[test]
fn m3_fork() {
    let env = Env::new("edit.json");
    let scope = scope();
    let (_, value) = env.json(&[
        "run",
        "edit",
        "--scope",
        scope.path().to_str().unwrap(),
        "--profile",
        "read_only",
        "--grant",
        "tool:write_file",
        "--grant",
        "tool:delete_path",
    ]);
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let source = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&[
        "task",
        "new",
        "continue",
        "--from-task",
        &source[..8],
        "--hold",
    ]);
    assert_eq!(code, 0, "{value}");
    let fork = value["task"]["task_id"].as_str().unwrap().to_owned();
    assert_eq!(value["task"]["forked_from"], source);
    assert_eq!(value["snapshots"][0]["reason"], "fork");
    assert_eq!(
        value["grants"].as_array().unwrap().len(),
        5,
        "the source's grants, copied"
    );
    assert_eq!(
        std::fs::read_to_string(env.tree(&fork).join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 1 }\n"
    );
    assert!(!env.tree(&fork).join("src/old.rs").exists());
    assert_eq!(
        std::fs::read_to_string(env.tree(&source).join("src/lib.rs")).unwrap(),
        "fn a() -> i32 { 1 }\n",
        "the source is untouched"
    );
    let (_, shown, _) = env.run(&["task", "show", &fork]);
    assert!(shown.contains("forked    from task"), "{shown}");
}
