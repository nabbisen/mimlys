#![cfg(target_os = "linux")]
// THROWAWAY measurement harness (handoff 011j); not committed.
use serde_json::{Value, json};
use std::process::Command;

#[test]
fn zz_measure() {
    let cmds: Vec<(String, bool, String)> =
        serde_json::from_str(&std::fs::read_to_string(std::env::var("ZZ_CMDS").unwrap()).unwrap())
            .unwrap();
    let data = tempfile::tempdir().unwrap();
    let script_path = data.path().join("script.json");
    let script: Vec<Value> = cmds
        .iter()
        .enumerate()
        .map(|(i, (_, net, c))| {
            json!({"decision":"act","tool":"run_host_tool","arguments":{"tool": if *net {"shnet"} else {"sh"}, "args":["-c", c]}})
        })
        .chain(std::iter::once(json!({"decision":"complete","summary":"done"})))
        .collect();
    std::fs::write(&script_path, serde_json::to_string(&script).unwrap()).unwrap();
    let config = data.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"config_version = 1
[runtime]
data_dir = {data:?}
[backend]
kind = "fake"
script = {script:?}
[policy.profiles.workspace]
tools = ["read_file", "write_file", "list_dir"]
[host_tools.sh]
command = "/bin/sh"
arg_pattern = ".*"
max_args = 2
allowed_args = ["-c"]
effect = "workspace_write"
timeout_ms = 60000
env_allow = ["PATH"]
read_paths = [{extra}]
[host_tools.shnet]
command = "/bin/sh"
arg_pattern = ".*"
max_args = 2
allowed_args = ["-c"]
effect = "external_read"
timeout_ms = 60000
network = true
env_allow = ["PATH"]
read_paths = [{extra}]
env = {{ {envs} }}
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script_path.to_string_lossy(),
            extra = std::env::var("ZZ_EXTRA").unwrap_or_default(),
            envs = std::env::var("ZZ_ENV").unwrap_or_default()
        ),
    )
    .unwrap();
    let scope = data.path().join("scope");
    std::fs::create_dir_all(&scope).unwrap();
    let run = |args: &[&str]| -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_mimlys"))
            .arg("--config")
            .arg(&config)
            .arg("--json")
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{e}: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        })
    };
    let args = vec![
        "run",
        "measure",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "host_tool:sh",
        "--grant",
        "host_tool:shnet",
    ];
    let v = run(&args);
    let id = v["task"]["task_id"].as_str().unwrap_or("?").to_owned();
    println!("TASK {id} state {}", v["task"]["state"]);
    if id == "?" {
        println!("{v}");
        return;
    }
    let conn = rusqlite::Connection::open(data.path().join("data/mimlys.db")).unwrap();
    let mut stmt = conn.prepare("select o.record from observations o join actions a on a.action_id = o.action_id where a.task_id = ?1 order by o.rowid").unwrap();
    let obs: Vec<Value> = stmt
        .query_map([&id], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    for (i, o) in obs.iter().enumerate() {
        let _ = i;
        println!("{}", o["stdout"].as_str().unwrap_or("").trim());
    }
    if obs.len() < cmds.len() {
        println!("ONLY {} OBSERVATIONS; value {v}", obs.len());
    }
}
