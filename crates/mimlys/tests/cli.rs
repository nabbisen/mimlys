//! The `mimlys` binary: configuration errors, exit codes, `--json` output
//! and the non-interactive paths of the commands (REQ-CLI-001, 004, 005;
//! REQ-CFG-001).

use serde_json::Value;
use std::path::Path;
use std::process::Command;

mod common;
use common::run_bounded;

struct Env {
    data: tempfile::TempDir,
    config: std::path::PathBuf,
}

impl Env {
    fn new(extra: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        let text = format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n{extra}",
            data.path().join("data").to_string_lossy()
        );
        std::fs::write(&config, text).unwrap();
        Self { data, config }
    }

    fn run(&self, args: &[&str], stdin: &str) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config").arg(&self.config).args(args);
        cmd.env("MIMLYS_CONFIG", "");
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        {
            use std::io::Write;
            let mut pipe = child.stdin.take().unwrap();
            let _ = pipe.write_all(stdin.as_bytes());
        }
        let out = child.wait_with_output().unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all, "");
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }

    /// A run bounded by `common::run_bounded` (no standard input): the
    /// exit code and both streams.
    fn bounded(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config").arg(&self.config).args(args);
        cmd.env("MIMLYS_CONFIG", "");
        let out = run_bounded(&mut cmd, self.data.path());
        (out.code(), out.stdout, out.stderr)
    }

    /// As [`Env::bounded`] with `--json`: the exit code and the value.
    fn bounded_json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.bounded(&all);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }
}

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "fn a() {}\n").unwrap();
    dir
}

fn demo_script() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/m1/demo.json")
        .to_string_lossy()
        .into_owned()
}

/// REQ-CFG-001: `check` validates; an unknown key exits 1 naming the key;
/// a usage error exits 2 without a stack trace.
#[test]
fn req_cfg_001_check_and_exit_codes() {
    let env = Env::new("");
    let (code, stdout, _) = env.run(&["check"], "");
    assert_eq!(code, 0);
    assert!(stdout.contains("configuration ok"));
    assert!(stdout.contains("config_version = 1"));
    let (code, value) = env.json(&["check"]);
    assert_eq!(code, 0);
    assert_eq!(value["ok"], true);
    assert_eq!(value["config"]["backend"]["kind"], "fake");

    let bad = Env::new("[backend]\nkind = \"fake\"\ncolour = \"blue\"\n");
    let (code, _, stderr) = bad.run(&["check"], "");
    assert_eq!(code, 1);
    assert!(stderr.contains("colour"), "{stderr}");
    assert!(!stderr.contains("panicked"));
    let (code, value) = bad.json(&["check"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["kind"], "invalid_request");

    let (code, _, stderr) = env.run(&["task"], "");
    assert_eq!(code, 2, "{stderr}");
    let (code, _, _) = env.run(&["--version"], "");
    assert_eq!(code, 0);
    let (code, _, stderr) = env.run(&["gc"], "");
    assert_eq!(
        code, 2,
        "gc never shipped: deletion is explicit (RFC 006); {stderr}"
    );
}

/// REQ-CLI-004: every command has a JSON form with stable fields; task
/// control from `created` through `halted` to `cancelled`.
#[test]
fn req_cli_004_json_and_task_control() {
    // The fake backend without a script: `check`, `status` and `--hold` work,
    // and a command that would run a step is refused before it starts.
    let bare = Env::new("");
    let scope = scope();
    let (code, created) = bare.json(&[
        "task",
        "new",
        "fix it",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    assert_eq!(code, 0, "{created}");
    let (code, refused) = bare.json(&[
        "task",
        "resume",
        created["task"]["task_id"].as_str().unwrap(),
    ]);
    assert_eq!(code, 1, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("backend.script"),
        "{refused}"
    );
    drop(bare);

    // An empty script: the backend answers nothing and the first step halts.
    let empty = scope.path().join("empty.json");
    std::fs::write(&empty, "[]").unwrap();
    let env = Env::new(&format!(
        "[backend]\nkind = \"fake\"\nscript = {:?}\n",
        empty.to_string_lossy()
    ));
    let (code, value) = env.json(&[
        "task",
        "new",
        "fix it",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "created");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    assert_eq!(
        value["grants"].as_array().unwrap().len(),
        8,
        "one grant per profile tool"
    );

    let (_, list) = env.json(&["task", "list"]);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["state"], "created");

    let (_, shown) = env.json(&["task", "show", &id[..8]]);
    assert_eq!(shown["task"]["task_id"], id, "a unique prefix resolves");

    // The backend runs out at once and the task halts with the reason.
    let (code, value) = env.json(&["task", "resume", &id]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "halted");
    assert_eq!(value["task"]["halt_reason"], "backend_unavailable");

    let (_, events) = env.json(&["events", &id]);
    let names: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["record"]["event"].as_str().unwrap())
        .collect();
    assert!(
        names.starts_with(&["task.created", "workspace.materialized"]),
        "{names:?}"
    );
    assert!(names.contains(&"step.started"));

    let (_, status) = env.json(&["status"]);
    // The tier is a real worker's report (RFC 011): `full` where every
    // mandatory clause is enforced, else `core`; either way the table says why.
    let tier = status["tier"].as_str().unwrap();
    assert!(tier == "core" || tier == "full", "{tier}");
    assert!(status["clauses"]["filesystem"]["state"].is_string());
    assert_eq!(status["backend"]["kind"], "fake");
    assert_eq!(status["backend"]["locality"], "local");
    assert_eq!(status["backend"]["reachable"], true);
    assert_eq!(status["backend"]["structured_output"], "none");
    assert!(status.get("backend_kind").is_none());
    assert!(status.get("backend_locality").is_none());
    assert!(status["data_dir"].as_str().unwrap().ends_with("data"));

    let (_, metrics) = env.json(&["metrics", "--task", &id]);
    assert_eq!(metrics["halts_by_reason"]["backend_unavailable"], 1);

    let (code, value) = env.json(&["task", "cancel", &id]);
    assert_eq!(code, 0);
    assert_eq!(value["task"]["state"], "cancelled");
    let (code, value) = env.json(&["task", "resume", &id]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["kind"], "state");

    let (code, value) = env.json(&["task", "show", "nope"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["kind"], "not_found");
}

/// REQ-CLI-003 / REQ-APR-007: without a terminal, `run` reports the
/// pending approval and exits 0; `reject --halt` from another process
/// ends the task `approval_rejected`; the scope is never written.
#[test]
fn req_cli_003_run_waits_without_a_terminal() {
    let env = Env::new(&format!(
        "[backend]\nkind = \"fake\"\nscript = {:?}\n[policy.profiles.workspace]\napproval_tools = [\"write_file\"]\n",
        demo_script()
    ));
    let scope = scope();
    let (code, stdout, stderr) = env.run(
        &["run", "fix it", "--scope", scope.path().to_str().unwrap()],
        "a\n",
    );
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("waiting for your approval"), "{stdout}");
    assert!(stdout.contains("approval.requested"), "{stdout}");
    assert!(
        stdout.contains("step.started"),
        "events are printed as they happen: {stdout}"
    );
    let (_, list) = env.json(&["task", "list"]);
    let id = list[0]["task_id"].as_str().unwrap().to_owned();
    assert_eq!(list[0]["state"], "waiting_user");

    let (_, shown, _) = env.run(&["task", "show", &id], "");
    assert!(shown.contains("Approval required"), "{shown}");
    assert!(shown.contains("write_file"), "{shown}");
    assert!(shown.contains("[a]pprove"), "{shown}");

    let (code, value) = env.json(&["reject", &id, "--halt", "--note", "no"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "halted");
    assert_eq!(value["task"]["halt_reason"], "approval_rejected");
    assert_eq!(
        std::fs::read_to_string(scope.path().join("src/lib.rs")).unwrap(),
        "fn a() {}\n"
    );
    assert!(
        env.data
            .path()
            .join("data/workspaces")
            .join(&id)
            .join("tree/src/lib.rs")
            .is_file()
    );
}

/// REQ-CAP-004 / REQ-CAP-005: grants and revocations from the command line.
#[test]
fn req_cap_004_grant_and_revoke() {
    let env = Env::new("");
    let scope = scope();
    let (_, value) = env.json(&[
        "task",
        "new",
        "g",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
        "--profile",
        "read_only",
    ]);
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    assert_eq!(value["grants"].as_array().unwrap().len(), 3);
    let (code, grant) = env.json(&["grant", &id, "tool", "write_file"]);
    assert_eq!(code, 0, "{grant}");
    assert_eq!(
        grant["kind"],
        serde_json::json!({"kind": "tool", "name": "write_file"})
    );
    let grant_id = grant["grant_id"].as_str().unwrap().to_owned();
    let (code, denied) = env.json(&["grant", &id, "network", "example.com"]);
    assert_eq!(code, 1);
    assert_eq!(denied["error"]["kind"], "denied", "outside the ceiling");
    let (code, revoked) = env.json(&["revoke", &id, &grant_id]);
    assert_eq!(code, 0, "{revoked}");
    assert_eq!(revoked["status"], "revoked");
    let (_, shown) = env.json(&["task", "show", &id]);
    assert_eq!(shown["grants"].as_array().unwrap().len(), 3);
}

/// REQ-CONF-011 / REQ-CFG-001: `mimlys check` and every command refuse a
/// declared read path that holds credentials, with exit code 1 and the
/// message naming the declaration, the entry and the narrower path; the
/// narrower path loads. And REQ-CONF-006: a dependency step declared as
/// `workspace_write` is refused with the value to write.
#[test]
fn req_conf_011_check_refuses_a_directory_that_holds_credentials() {
    let toolchain = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(toolchain.path().join("bin")).unwrap();
    std::fs::write(toolchain.path().join("credentials.toml"), "x").unwrap();
    let declare = |path: &Path, tail: &str| {
        format!(
            "[host_tools.cargo_test]\ncommand = \"/bin/true\"\neffect = \"workspace_write\"\nread_paths = [{:?}]\n{tail}",
            path.to_string_lossy()
        )
    };
    let env = Env::new(&declare(toolchain.path(), ""));
    let (code, value) = env.json(&["check"]);
    assert_eq!(code, 1, "{value}");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("host_tools.cargo_test.read_paths"),
        "{message}"
    );
    assert!(message.contains("credentials.toml"), "{message}");
    assert!(
        message.contains(&format!("{}/bin", toolchain.path().display())),
        "{message}"
    );
    // The narrower path loads.
    let env = Env::new(&declare(&toolchain.path().join("bin"), ""));
    let (code, value) = env.json(&["check"]);
    assert_eq!(code, 0, "{value}");
    // The dependency step with the wrong effect names the value to write.
    let env = Env::new(&declare(&toolchain.path().join("bin"), "network = true\n"));
    let (code, value) = env.json(&["check"]);
    assert_eq!(code, 1, "{value}");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("host_tools.cargo_test.effect"),
        "{message}"
    );
    assert!(message.contains("external_read"), "{message}");
}

/// REQ-CONF-006 (no shell is involved), REQ-CFG-001: a declaration whose
/// `command` is a batch script, `.cmd` or `.bat` in any case, is refused by
/// `check` and at load by every command, on every platform, with exit code 1
/// and the message naming the declaration, the reason and the remedy; the
/// program the script runs loads. The Windows job runs this too.
#[test]
fn req_conf_006_check_refuses_a_batch_script_command() {
    for command in ["x.cmd", "X.BAT", "tools/run.Cmd"] {
        let env = Env::new(&format!(
            "[host_tools.build]\ncommand = {command:?}\neffect = \"workspace_write\"\n"
        ));
        for args in [&["check"][..], &["status"][..]] {
            let (code, value) = env.json(args);
            assert_eq!(code, 1, "{command} {args:?}: {value}");
            let message = value["error"]["message"].as_str().unwrap();
            assert!(
                message.contains("host_tools.build.command")
                    && message.contains("no shell is involved")
                    && message.contains("declare the program the script runs"),
                "{command} {args:?}: {message}"
            );
        }
    }
    let env = Env::new("[host_tools.build]\ncommand = \"x.exe\"\neffect = \"workspace_write\"\n");
    let (code, value) = env.json(&["check"]);
    assert_eq!(code, 0, "the program the script runs loads: {value}");
}

/// REQ-CONF-008: `toolchain revoke --all` takes no path (a usage error, exit
/// 2), and one of the two is required.
#[test]
fn req_conf_008_revoke_all_with_a_path_is_a_usage_error() {
    let env = Env::new("");
    let (code, _, stderr) = env.run(&["toolchain", "revoke", "--all", "somewhere", "--yes"], "");
    assert_eq!(code, 2, "{stderr}");
    let (code, _, stderr) = env.run(&["toolchain", "revoke", "--yes"], "");
    assert_eq!(code, 2, "a path or --all is required: {stderr}");
}

/// REQ-CONF-008: where no directory permission was ever applied (Linux,
/// macOS) `revoke --all` says nothing was changed outside the data directory
/// and exits 0, with the documented JSON shape.
#[cfg(not(windows))]
#[test]
fn req_conf_008_revoke_all_elsewhere_says_nothing_changed() {
    let env = Env::new("");
    let (code, stdout, stderr) = env.run(&["toolchain", "revoke", "--all", "--yes"], "");
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        stdout.contains("nothing was changed outside the data directory"),
        "{stdout}"
    );
    let (code, value) = env.json(&["toolchain", "revoke", "--all", "--yes"]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(
        value,
        serde_json::json!({"removed": [], "failed": [], "profile_deleted": false, "record_deleted": false})
    );
}

/// REQ-WS-001 (external design §14.1): a task's workspace is created under
/// `<data_dir>/workspaces/<task_id>/`, and the data directory, which did not
/// exist, is created owner-only: on Unix mode 0700, and `workspaces/`,
/// `tmp/` and `interpreters/` in it 0700 too; on Windows a DACL with one
/// entry, the current user's full control, none inherited. The data
/// directory of this test is a path that did not exist, so the protection is
/// the runtime's own and not the temporary directory's. (The default
/// location, outside any temporary directory, is
/// `req_ws_001_default_data_dir_is_not_temporary` in the configuration
/// tests.)
#[test]
fn req_ws_001_workspace_under_the_owner_only_data_directory() {
    let env = Env::new("");
    let data_dir = env.data.path().join("data");
    assert!(
        !data_dir.exists(),
        "the data directory is the runtime's to create"
    );
    let scope = scope();
    let (code, created) = env.bounded_json(&[
        "task",
        "new",
        "fix it",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    assert_eq!(code, 0, "{created}");
    let id = created["task"]["task_id"].as_str().unwrap();
    let tree = data_dir.join("workspaces").join(id).join("tree");
    assert!(tree.join("src/lib.rs").is_file(), "{}", tree.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        for dir in ["", "workspaces", "tmp", "interpreters"] {
            assert_eq!(mode(&data_dir.join(dir)), 0o700, "{dir:?}");
        }
    }
    #[cfg(windows)]
    {
        let icacls = Command::new("icacls").arg(&data_dir).output().unwrap();
        assert!(icacls.status.success());
        let text = String::from_utf8_lossy(&icacls.stdout).into_owned();
        let shown = data_dir.to_string_lossy().into_owned();
        let entries: Vec<String> = text
            .lines()
            .map(|l| {
                l.strip_prefix(shown.as_str())
                    .unwrap_or(l)
                    .trim()
                    .to_lowercase()
            })
            .filter(|l| l.contains(":("))
            .collect();
        let me = Command::new("whoami").output().unwrap();
        let me = String::from_utf8_lossy(&me.stdout).trim().to_lowercase();
        assert_eq!(entries.len(), 1, "{text}");
        assert!(entries[0].starts_with(&format!("{me}:")), "{text}");
        assert!(
            entries[0].contains("(f)") && !entries[0].contains("(i)"),
            "{text}"
        );
    }
}

/// REQ-SEC-003 (external design §16.2, §21): an error in persistence results
/// in no execution. After the task is created, a trigger in the database
/// makes the write of the `running` line fail, as a full disk or a broken
/// file would. The model's `write_file` is authorized and then not run: the
/// command fails, no execution row exists and the file is nowhere.
#[test]
fn req_sec_003_a_persistence_error_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.json");
    std::fs::write(
        &script,
        r#"[{"decision":"act","tool":"write_file","arguments":{"path":"evidence.txt","content":"ran"}},
            {"decision":"complete","summary":"done"}]"#,
    )
    .unwrap();
    let config = format!(
        "[backend]\nkind = \"fake\"\nscript = {:?}\n",
        script.to_string_lossy()
    );
    let scope = scope();
    // The control: without the failure the same script writes the file.
    let control = Env::new(&config);
    let (_, created) = control.bounded_json(&[
        "task",
        "new",
        "write the evidence",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    let control_id = created["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, _, stderr) = control.bounded(&["task", "resume", &control_id]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        control
            .data
            .path()
            .join("data/workspaces")
            .join(&control_id)
            .join("tree/evidence.txt")
            .is_file(),
        "the control writes the file"
    );

    let env = Env::new(&config);
    let (code, created) = env.bounded_json(&[
        "task",
        "new",
        "write the evidence",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    assert_eq!(code, 0, "{created}");
    let id = created["task"]["task_id"].as_str().unwrap().to_owned();

    let db = env.data.path().join("data/mimlys.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER injected_failure BEFORE UPDATE ON actions
             WHEN NEW.status = 'running'
             BEGIN SELECT RAISE(ABORT, 'injected persistence failure'); END;",
        )
        .unwrap();
    }

    let (code, stdout, stderr) = env.bounded(&["task", "resume", &id]);
    assert_ne!(code, 0, "{stdout}\n{stderr}");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let executions: i64 = conn
        .query_row("SELECT count(*) FROM executions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(executions, 0, "no execution was recorded");
    let status: String = conn
        .query_row("SELECT status FROM actions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "authorized", "the action never reached `running`");
    let tree = env
        .data
        .path()
        .join("data/workspaces")
        .join(&id)
        .join("tree");
    assert!(
        !tree.join("evidence.txt").exists(),
        "the tool did not run in the workspace"
    );
    assert!(!scope.path().join("evidence.txt").exists());
}

/// REQ-WS-001 (external design §14.1): a configured `runtime.data_dir` inside
/// the temporary directory is named by `check` and `status`, in one line
/// that names the key and the risk (a cleaner of temporary files can delete
/// tasks), in the JSON output as `data_dir_in_temporary_directory`, and is
/// not refused: both commands succeed (`status` reads the directory, and creates
/// nothing in it: REQ-PERS-013). A
/// data directory outside it is not named.
#[test]
fn req_ws_001_a_data_directory_in_the_temporary_directory_is_warned_about_not_refused() {
    // `Env` puts the data directory under `tempfile::tempdir()`.
    let env = Env::new("");
    let data_dir = env.data.path().join("data");
    let warning_lines = |stdout: &str| -> Vec<String> {
        stdout
            .lines()
            .filter(|l| l.contains("runtime.data_dir"))
            .map(str::to_owned)
            .collect()
    };
    for command in ["check", "status"] {
        let (code, stdout, stderr) = env.bounded(&[command]);
        assert_eq!(code, 0, "{command}: {stdout}\n{stderr}");
        let lines = warning_lines(&stdout);
        assert_eq!(lines.len(), 1, "{command}: {stdout}");
        assert!(lines[0].starts_with("warning:"), "{}", lines[0]);
        assert!(
            lines[0].contains("temporary directory")
                && lines[0].contains("a cleaner of temporary files can delete tasks"),
            "{}",
            lines[0]
        );
        let (code, value) = env.bounded_json(&[command]);
        assert_eq!(code, 0, "{value}");
        assert_eq!(value["data_dir_in_temporary_directory"], true, "{value}");
    }
    // REQ-PERS-013: `status` only reads; it creates nothing in the directory it
    // warns about. A command that writes is what uses it.
    assert!(
        !data_dir.exists(),
        "status and check create no data directory"
    );
    let (code, stdout, stderr) = env.bounded(&["interpreters", "list"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    assert!(
        data_dir.join("mimlys.db").is_file(),
        "a command that writes used the directory"
    );

    // Outside the temporary directory: not named. `check` creates nothing.
    let outside = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-data-directory");
    let inside_temp = |p: &Path| {
        p.starts_with(std::env::temp_dir().canonicalize().unwrap())
            || p.starts_with(std::env::temp_dir())
    };
    if inside_temp(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .unwrap(),
    ) {
        common::skip(
            "req_ws_001_a_data_directory_in_the_temporary_directory_is_warned_about_not_refused",
            "the repository itself lies inside the temporary directory",
        );
        return;
    }
    let config = env.data.path().join("outside.toml");
    std::fs::write(
        &config,
        format!(
            "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
            outside.to_string_lossy()
        ),
    )
    .unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.arg("--config").arg(&config).arg("check");
    cmd.env("MIMLYS_CONFIG", "");
    let out = run_bounded(&mut cmd, env.data.path());
    assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
    assert!(warning_lines(&out.stdout).is_empty(), "{}", out.stdout);
    assert!(!outside.exists(), "check creates no directory");
}

/// REQ-SBX-001 (external design §12 and §18): `interpreters install` against a
/// server that answers 404: not retried (one request), and the JSON error is
/// `unavailable` with `component: "download"`, naming the URL, the status and
/// the `--from FILE` remedy; the exit code is not 0. The address of the
/// pinned artifact is replaced by the debug-only variable
/// `MIMLYS_DEBUG_INTERPRETER_URL`, absent from release builds.
#[cfg(debug_assertions)]
#[test]
fn req_sbx_001_a_failed_download_is_reported_as_unavailable_download() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/a.wasm", listener.local_addr().unwrap());
    let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = std::sync::Arc::clone(&served);
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\nno",
            );
        }
    });
    let env = Env::new("");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.arg("--config")
        .arg(&env.config)
        .args(["--json", "interpreters", "install", "javascript"])
        .env("MIMLYS_CONFIG", "")
        .env("MIMLYS_DEBUG_INTERPRETER_URL", &url);
    let out = run_bounded(&mut cmd, env.data.path());
    assert_ne!(out.code(), 0, "{}", out.stderr);
    let json_line = |text: &str| {
        text.lines()
            .rev()
            .find(|l| l.starts_with('{'))
            .map(str::to_owned)
    };
    let line = json_line(&out.stdout)
        .or_else(|| json_line(&out.stderr))
        .unwrap_or_else(|| panic!("no JSON: {}\n{}", out.stdout, out.stderr));
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["error"]["kind"], "unavailable", "{value}");
    assert_eq!(value["error"]["detail"]["component"], "download", "{value}");
    assert_eq!(value["error"]["detail"]["last_status"], 404, "{value}");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains(&url), "{message}");
    assert!(message.contains("--from FILE"), "{message}");
    assert_eq!(
        served.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a 404 is not retried"
    );
}

/// REQ-MAIN-002, REQ-PERS-012 (handoff 004b): a task records the decision, event and action
/// schema versions in force at creation, and `task show` gives them back.
#[test]
fn req_main_002_a_task_records_the_three_schema_versions_in_the_cli() {
    let env = Env::new("");
    let scope = scope();
    let (code, created) = env.json(&[
        "task",
        "new",
        "versions",
        "--scope",
        scope.path().to_str().unwrap(),
        "--hold",
    ]);
    assert_eq!(code, 0, "{created}");
    let id = created["task"]["task_id"].as_str().unwrap().to_owned();
    for value in [
        created["task"].clone(),
        env.json(&["task", "show", &id]).1["task"].clone(),
    ] {
        let versions = &value["versions"];
        assert_eq!(versions["decision_schema_version"], 1, "{versions}");
        assert_eq!(
            versions["event_schema_version"],
            mimlys_core::event::EVENT_SCHEMA_VERSION,
            "{versions}"
        );
        assert_eq!(
            versions["action_schema_version"],
            mimlys_core::action::ACTION_SCHEMA_VERSION,
            "{versions}"
        );
    }
}
