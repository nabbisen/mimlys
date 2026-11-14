//! M5 tests (RFC 009; REQ-TEST-012, REQ-TEST-015) over the scripted MCP
//! test server (`mimlys-mcp-testserver`, which serves rypsok 0.9.0's tools
//! from rypsok's committed transcripts): the demonstration of external
//! design §23 (M5), error mapping, environment scrubbing, pinning, output
//! validation, cancellation, death and restarts, undeclared defaults, and
//! the configuration surface. With `MIMLYS_RYPSOK_BIN` set, the
//! demonstration also runs against a real `rypsok serve`.

use mimlys_core::spi::Cancellation;
use mimlys_mcp::client::CallOutcome;
use mimlys_mcp::server::{Server, ServerConfig};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

mod common;
use common::run_bounded;

fn testserver() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mimlys-mcp-testserver"))
}

const BUILTIN: &str = r#""read_file", "write_file", "replace_in_file", "list_dir", "search_files", "delete_path", "run_script", "run_wasm""#;

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
    extra_env: Vec<(String, String)>,
}

impl Env {
    /// A configuration with the test server as `rypsok`, its three tools
    /// declared `external_read`, `url` a destination, and the controls
    /// admitted through the ceiling and the profile.
    fn new(script: &[Value], extra_server: &str, extra_profile_tools: &[&str]) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        let script_path = data.path().join("script.json");
        std::fs::write(&script_path, serde_json::to_string(script).unwrap()).unwrap();
        let mut profile_tools = vec![
            "rypsok.web_search",
            "rypsok.web_fetch",
            "rypsok.echo_env",
            "rypsok.slow",
            "rypsok.change_definition",
        ];
        profile_tools.extend(extra_profile_tools);
        let externals: Vec<String> = profile_tools
            .iter()
            .chain(["rypsok.engine_status", "rypsok.die"].iter())
            .map(|t| format!("{t:?}"))
            .collect();
        let text = format!(
            r#"config_version = 1
[runtime]
data_dir = {data:?}
[backend]
kind = "fake"
script = {script:?}
[policy.ceiling]
tools = [{builtin}, {externals}]
network_domains = ["example.org", "en.wikipedia.org"]
[policy.profiles.workspace]
tools = [{builtin}, {profile}]
[servers.rypsok]
command = {server:?}
env_allow = ["PATH", "MIMLYS_TESTSERVER_HOSTILE", "MIMLYS_TESTSERVER_BAD_OUTPUT", "MIMLYS_TESTSERVER_UNSUPPORTED", "MIMLYS_TESTSERVER_WIDE", "MIMLYS_TESTSERVER_IGNORE_CANCEL", "MIMLYS_TESTSERVER_DIE_LOG"]
{extra_server}
[servers.rypsok.tools.web_search]
effect = "external_read"
idempotent = true
timeout_ms = 10000
[servers.rypsok.tools.web_fetch]
effect = "external_read"
idempotent = true
destination_arguments = ["url"]
timeout_ms = 15000
[servers.rypsok.tools.echo_env]
effect = "external_read"
idempotent = true
[servers.rypsok.tools.slow]
effect = "external_read"
idempotent = true
timeout_ms = 3000
[servers.rypsok.tools.change_definition]
effect = "external_read"
idempotent = true
"#,
            data = data.path().join("data").to_string_lossy(),
            script = script_path.to_string_lossy(),
            builtin = BUILTIN,
            externals = externals.join(", "),
            profile = profile_tools
                .iter()
                .map(|t| format!("{t:?}"))
                .collect::<Vec<_>>()
                .join(", "),
            server = testserver().to_string_lossy(),
        );
        std::fs::write(&config, text).unwrap();
        Self {
            data,
            config,
            extra_env: vec![],
        }
    }

    fn env(mut self, key: &str, value: &str) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .env("BRAVE_API_KEY", "operator-secret-brave-1234");
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
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

    fn scope(&self) -> PathBuf {
        let dir = self.data.path().join("scope");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn a() {}\n").unwrap();
        dir
    }

    fn events(&self, id: &str) -> Vec<Value> {
        let (_, events) = self.json(&["events", id]);
        events
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["record"].clone())
            .collect()
    }
}

fn act(tool: &str, args: Value) -> Value {
    json!({"decision": "act", "tool": tool, "arguments": args})
}

fn complete() -> Value {
    json!({"decision": "complete", "summary": "done"})
}

fn names(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| e["event"].as_str().map(str::to_owned))
        .collect()
}

/// External design §23, M5: `rypsok.web_search` and `rypsok.web_fetch` run
/// under a `network` grant; a fetch to an ungranted host is `denied`
/// before anything leaves the host; `rypsok.engine_status` is not listed
/// to the model; every imported definition is pinned; the server's start
/// is in the audit.
/// INV-13 (external design §21.2): external network only through a matching
/// `network` grant; the fetch to a host outside the grant is `denied` and the
/// server is called twice, not three times. REQ-MCP-009 / K-04: an imported
/// tool with no `tool` grant (`rypsok.engine_status` is in the ceiling but in
/// no profile) is imported and pinned but invisible to the model: its
/// proposal is rejected `unknown_tool`.
#[test]
fn m5_search_and_fetch_under_a_network_grant() {
    let env = Env::new(
        &[
            act(
                "rypsok.web_search",
                json!({"query": "rust programming language", "max_results": 5}),
            ),
            act(
                "rypsok.web_fetch",
                json!({"url": "https://example.org/article"}),
            ),
            act(
                "rypsok.web_fetch",
                json!({"url": "https://other.example/page"}),
            ),
            act("rypsok.engine_status", json!({})),
            complete(),
        ],
        "",
        &[],
    );
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "search then read",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "network:example.org",
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let imports = &value["task"]["versions"]["tool_imports"];
    assert_eq!(imports["rypsok.web_search"]["server"], "rypsok");
    assert_eq!(imports["rypsok.web_search"]["declared"], true);
    assert_eq!(
        imports["rypsok.web_search"]["annotations"]["readOnlyHint"],
        true
    );
    assert_eq!(imports["rypsok.engine_status"]["declared"], false);
    assert!(imports["rypsok.web_search"]["unavailable"].is_null());

    let events = env.events(&id);
    let n = names(&events);
    let pinned: Vec<&str> = events
        .iter()
        .filter(|e| e["event"] == "tool.pinned")
        .filter_map(|e| e["tool"].as_str())
        .collect();
    assert!(pinned.contains(&"rypsok.web_search"), "{pinned:?}");
    assert!(pinned.contains(&"rypsok.web_fetch"), "{pinned:?}");
    assert!(pinned.contains(&"rypsok.engine_status"), "{pinned:?}");
    let finished: Vec<String> = events
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| e["status"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(finished, ["succeeded", "succeeded"], "{n:?}");
    let rejected: Vec<String> = events
        .iter()
        .filter(|e| e["event"] == "decision.rejected")
        .map(|e| e["code"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(rejected, ["denied", "unknown_tool"], "{n:?}");
    assert!(!n.contains(&"tool.unpinned".to_owned()));
    let (_, all) = env.json(&["metrics"]);
    assert!(all["events_by_name"]["server.started"].as_u64().unwrap() >= 1);
    assert!(
        all["events_by_name"]["server.stopped"].as_u64().unwrap() >= 1,
        "{}",
        all["events_by_name"]
    );

    let (_, status) = env.json(&["status"]);
    let server = &status["servers"][0];
    assert_eq!(server["name"], "rypsok");
    assert_eq!(
        server["state"], "running",
        "status starts the server to list its tools"
    );
    assert_eq!(server["server_name"], "mimlys-testserver");
    assert_eq!(server["list_changed"], false);
    let tools: Vec<&str> = server["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(tools.contains(&"rypsok.web_search"), "{tools:?}");
    assert!(
        tools.contains(&"rypsok.engine_status"),
        "imported, invisible to the model until granted"
    );
    assert!(
        server["instructions"]
            .as_str()
            .unwrap()
            .contains("external data")
    );
}

/// RFC 009 §6: rypsok's error kinds reach the model as `tool_error{kind}`
/// with the error object as payload; a plain-http URL is `denied` at
/// stage 7 before any call; a redirect is never followed.
#[test]
fn m5_errors_are_mapped_by_kind() {
    let env = Env::new(
        &[
            act(
                "rypsok.web_fetch",
                json!({"url": "https://example.org/moved"}),
            ),
            act(
                "rypsok.web_search",
                json!({"query": "my password is hunter2"}),
            ),
            act(
                "rypsok.web_fetch",
                json!({"url": "http://192.168.1.10/admin"}),
            ),
            complete(),
        ],
        "",
        &[],
    );
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "errors",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "network:example.org",
    ]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let events = env.events(&id);
    let finished: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| {
            (
                e["status"].as_str().unwrap().to_owned(),
                e["cause"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    assert_eq!(
        finished,
        [
            ("failed".to_owned(), "tool_error".to_owned()),
            ("failed".to_owned(), "tool_error".to_owned())
        ],
        "{events:?}"
    );
    let rejected: Vec<String> = events
        .iter()
        .filter(|e| e["event"] == "decision.rejected")
        .map(|e| e["code"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(rejected, ["denied"]);
}

/// REQ-PRIV-003 / REQ-TEST-015 / THR-24 / INV-14 (external design §21.2):
/// the child sees only `env_allow` and `env_secrets`; the operator's key is
/// absent from its environment; the secret's value is redacted from the
/// observation (which is what the model reads) and appears nowhere in the
/// event log, in the command's output or in its error stream.
#[test]
fn m5_environment_is_scrubbed_and_secrets_redacted() {
    let env = Env::new(
        &[act("rypsok.echo_env", json!({})), complete()],
        "env_secrets = { TOKEN = \"env:MIMLYS_TEST_TOKEN\" }",
        &[],
    )
    .env("MIMLYS_TEST_TOKEN", "tok-9f8e7d6c5b4a");
    let scope = env.scope();
    let (code, stdout, stderr) =
        env.run(&["--json", "run", "env", "--scope", scope.to_str().unwrap()]);
    let value: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
    assert_eq!(code, 0, "{value}");
    assert!(!stdout.contains("tok-9f8e7d6c5b4a"), "the command's output");
    assert!(!stderr.contains("tok-9f8e7d6c5b4a"), "the error stream");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let events = serde_json::to_string(&env.events(&id)).unwrap();
    assert!(!events.contains("tok-9f8e7d6c5b4a"), "the event log");
    let db = env.data.path().join("data/mimlys.db");
    let observation = read_observation(&db, &id);
    let vars = observation["payload"]["env"].as_object().unwrap();
    assert!(vars.contains_key("PATH"));
    assert_eq!(vars["TOKEN"], "[redacted:servers.rypsok.env.TOKEN]");
    assert!(!vars.contains_key("BRAVE_API_KEY"), "{vars:?}");
    assert!(!vars.contains_key("MIMLYS_TEST_TOKEN"));
    assert!(
        observation["notices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["code"] == "secret_redacted" && n["name"] == "servers.rypsok.env.TOKEN"),
        "{}",
        observation["notices"]
    );
    let text = serde_json::to_string(&observation).unwrap();
    assert!(!text.contains("tok-9f8e7d6c5b4a"));
}

fn read_observation(db: &std::path::Path, task_id: &str) -> Value {
    let conn = rusqlite::Connection::open(db).unwrap();
    let text: String = conn
        .query_row(
            "select o.record from observations o join actions a on a.action_id = o.action_id where a.task_id = ?1 order by o.rowid limit 1",
            [task_id],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// REQ-MCP-002 / THR-25: a changed definition is unpinned once at the next
/// boundary, named to the model, and hidden; a proposal of it is
/// `unknown_tool`; `status` says the list changed; a new task pins the new hash.
#[test]
fn m5_pinning_breaks_visibly() {
    let env = Env::new(
        &[
            act("rypsok.change_definition", json!({})),
            act("rypsok.web_search", json!({"query": "after the change"})),
            complete(),
        ],
        "",
        &[],
    );
    let scope = env.scope();
    let (code, value) = env.json(&["run", "pin", "--scope", scope.to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let old_hash = value["task"]["versions"]["tool_imports"]["rypsok.web_search"]["hash"].clone();
    let events = env.events(&id);
    let n = names(&events);
    assert!(n.contains(&"tool.unpinned".to_owned()), "{n:?}");
    assert_eq!(n.iter().filter(|e| *e == "tool.unpinned").count(), 1);
    let unpinned: Vec<&str> = events
        .iter()
        .filter(|e| e["event"] == "tool.unpinned")
        .filter_map(|e| e["tool"].as_str())
        .collect();
    assert_eq!(unpinned, ["rypsok.web_search"]);
    let rejected: Vec<String> = events
        .iter()
        .filter(|e| e["event"] == "decision.rejected")
        .map(|e| e["code"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(rejected, ["unknown_tool"]);
    // A new task pins the changed definition under a new hash (the server
    // restarts per process, so the change is re-issued first).
    let env2 = Env::new(
        &[act("rypsok.change_definition", json!({})), complete()],
        "",
        &[],
    );
    let scope2 = env2.scope();
    let (_, v1) = env2.json(&["run", "first", "--scope", scope2.to_str().unwrap()]);
    let (_, status) = env2.json(&["status"]);
    let _ = status;
    let first_hash = v1["task"]["versions"]["tool_imports"]["rypsok.web_search"]["hash"].clone();
    assert_eq!(
        first_hash, old_hash,
        "same definition, same hash across processes"
    );
}

/// REQ-TOOL-005 / REQ-MCP-006: a result that violates the pinned output
/// schema is `output_invalid` and never passed; a hostile description is
/// truncated (recorded on the task); unsupported and wide schemas are
/// unavailable with their reasons in `status`.
/// REQ-SEC-013 (hostile corpus, repository guards §3a), hostile description and bad output of an external server.
#[test]
fn m5_output_validation_and_import_reasons() {
    let env = Env::new(
        &[act("rypsok.web_search", json!({"query": "x"})), complete()],
        "",
        &[],
    )
    .env("MIMLYS_TESTSERVER_BAD_OUTPUT", "1")
    .env("MIMLYS_TESTSERVER_HOSTILE", "1")
    .env("MIMLYS_TESTSERVER_UNSUPPORTED", "1")
    .env("MIMLYS_TESTSERVER_WIDE", "1");
    let scope = env.scope();
    let (code, value) = env.json(&["run", "bad", "--scope", scope.to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let imports = &value["task"]["versions"]["tool_imports"];
    assert_eq!(imports["rypsok.web_search"]["description_truncated"], true);
    assert_eq!(
        imports["rypsok.odd"]["unavailable"],
        "schema_unsupported:x:anyOf"
    );
    assert!(
        imports["rypsok.wide"]["unavailable"]
            .as_str()
            .unwrap()
            .starts_with("too_many_parameters")
    );
    let events = env.events(&id);
    let finished: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| {
            (
                e["status"].as_str().unwrap().to_owned(),
                e["cause"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    assert_eq!(
        finished,
        [("failed".to_owned(), "output_invalid".to_owned())],
        "{:?}",
        names(&events)
    );
    let (_, status) = env.json(&["status"]);
    let unavailable = status["servers"][0]["unavailable"].as_array().unwrap();
    assert!(
        unavailable.iter().any(|u| u[0] == "rypsok.odd"),
        "{unavailable:?}"
    );
}

/// REQ-MCP-004 / THR-26: an undeclared tool is `external_write` and needs
/// approval even though the server marks it read-only.
#[test]
fn m5_undeclared_tool_needs_approval() {
    let env = Env::new(
        &[act("rypsok.engine_status", json!({})), complete()],
        "",
        &["rypsok.engine_status"],
    );
    let scope = env.scope();
    let (code, value) = env.json(&["run", "status", "--scope", scope.to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "waiting_user", "{value}");
    assert!(value["pending_approval"].is_object(), "{value}");
    assert!(
        value["pending_approval"]
            .to_string()
            .contains("engine_status"),
        "{value}"
    );
}

/// REQ-MCP-007 / REQ-MCP-001 (library level): cancellation sends the
/// notification and ends the call within the grace period; a server that
/// ignores it is killed; the deadline is `timeout`; a dead server is
/// restarted up to LIM-X3 times per task, then fails.
#[test]
fn m5_cancellation_deadline_death_and_restarts() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut config = ServerConfig::new("t", testserver(), vec![]);
        config.env_allow = vec!["PATH".into()];
        let server = Server::new(config);
        let task = mimlys_core::ids::TaskId::from_uuid(uuid::Uuid::now_v7());
        let (info, tools, _) = server.list().await.unwrap();
        assert_eq!(info.name.as_deref(), Some("mimlys-testserver"));
        assert!(tools.iter().any(|t| t.name == "web_search"));
        assert_eq!(
            info.instructions.as_deref(),
            Some(mimlys_mcp::testserver::INSTRUCTIONS)
        );

        // The tool holds for 60 s, far longer than anything the test waits for
        // (the rule of handoff 011i): the call ends because it was cancelled, or
        // because its deadline passed, never because the tool answered. The
        // outcome is the state that proves it; the time allowed is a margin, five
        // times LIM-O2 for the cancel (a cancel that needs a multiple of the limit
        // is a defect, a stall of a slow machine is not), half the hold for the
        // deadline of 300 ms (which is the behaviour under test).
        let cancel = Cancellation::new();
        let flag = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            flag.cancel();
        });
        let started = std::time::Instant::now();
        let outcome = server
            .call(
                task,
                "slow",
                json!({"ms": 60_000}).as_object().unwrap().clone(),
                120_000,
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(outcome, CallOutcome::Cancelled);
        assert!(
            started.elapsed()
                < std::time::Duration::from_millis(5 * mimlys_core::limits::LIM_O2_CANCEL_MS),
            "{:?}",
            started.elapsed()
        );

        let started = std::time::Instant::now();
        let outcome = server
            .call(
                task,
                "slow",
                json!({"ms": 60_000}).as_object().unwrap().clone(),
                300,
                &Cancellation::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome, CallOutcome::Timeout);
        assert!(started.elapsed() < std::time::Duration::from_secs(30));

        // Death and restarts: LIM-X3 per task. The deadlines of these calls are
        // not what is tested (a server that dies ends its call at once, a restarted
        // one answers at once): 30 s, so that a slow start is not a timeout.
        for i in 0..mimlys_core::limits::LIM_X3_RESTARTS {
            let outcome = server
                .call(
                    task,
                    "die",
                    Default::default(),
                    30_000,
                    &Cancellation::new(),
                )
                .await;
            assert!(
                matches!(outcome, Ok(CallOutcome::Failed { .. })),
                "{i}: {outcome:?}"
            );
            let again = server
                .call(
                    task,
                    "engine_status",
                    Default::default(),
                    30_000,
                    &Cancellation::new(),
                )
                .await
                .unwrap();
            assert!(
                matches!(again, CallOutcome::Result { .. }),
                "{i}: restarted"
            );
        }
        let _ = server
            .call(
                task,
                "die",
                Default::default(),
                30_000,
                &Cancellation::new(),
            )
            .await;
        let exhausted = server
            .call(
                task,
                "engine_status",
                Default::default(),
                30_000,
                &Cancellation::new(),
            )
            .await;
        assert!(
            matches!(exhausted, Err(mimlys_mcp::server::ServerError::Died { .. })),
            "{exhausted:?}"
        );
        let events = server.drain_events();
        let starts = events
            .iter()
            .filter(|e| e.name() == "server.started")
            .count();
        let stops = events
            .iter()
            .filter(|e| e.name() == "server.stopped")
            .count();
        assert!(starts >= 4 && stops >= 4, "{starts} starts, {stops} stops");
        server.shutdown().await;
    });

    // A server that ignores cancellation is killed after the grace period.
    rt.block_on(async {
        let mut config = ServerConfig::new("t", testserver(), vec![]);
        config.env_allow = vec!["PATH".into()];
        config.env_secrets = vec![("MIMLYS_TESTSERVER_IGNORE_CANCEL".into(), "1".into())];
        let server = Server::new(config);
        let task = mimlys_core::ids::TaskId::from_uuid(uuid::Uuid::now_v7());
        let cancel = Cancellation::new();
        cancel.cancel();
        let started = std::time::Instant::now();
        // The server hangs for 60 s (and has a deadline of 120 s): `Killed` is the
        // state that proves it was killed after the grace, not that it answered or
        // timed out; the time allowed is five times LIM-O2.
        let outcome = server
            .call(
                task,
                "slow",
                json!({"ms": 60_000}).as_object().unwrap().clone(),
                120_000,
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(outcome, CallOutcome::Killed);
        assert!(
            started.elapsed()
                < std::time::Duration::from_millis(5 * mimlys_core::limits::LIM_O2_CANCEL_MS),
            "{:?}",
            started.elapsed()
        );
        assert!(!server.is_running().await);
        server.shutdown().await;
    });
}

/// The `die` tool declared with `effect`, proposed `proposals` times: the server
/// process ends at each call, and each `die` that reaches the server is a line
/// in the returned log.
fn dying_env(effect: &str, proposals: usize) -> (Env, PathBuf) {
    let script: Vec<Value> = (0..proposals)
        .map(|_| act("rypsok.die", json!({})))
        .chain([complete()])
        .collect();
    let env = Env::new(
        &script,
        &format!(
            "[servers.rypsok.tools.die]\neffect = \"{effect}\"\nidempotent = {}\ntimeout_ms = 30000\n",
            effect == "external_read"
        ),
        &["rypsok.die"],
    );
    let log = env.data.path().join("die.log");
    (
        env.env("MIMLYS_TESTSERVER_DIE_LOG", log.to_str().unwrap()),
        log,
    )
}

fn die_calls(log: &std::path::Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |t| t.lines().count())
}

/// REQ-TOOL-009, REQ-MCP-001 (handoff 010e-5, item 1; external design §11.1):
/// every restart made for a task counts against LIM-X3, the one the pin check
/// makes before a call included. A server whose `die` tool (declared
/// `external_read` and idempotent) ends the process at each call is restarted
/// LIM-X3 times; the attempt after that is not made: `host_fault{unavailable}`,
/// the cause `tool_unavailable` with the reason `restart_limit_reached`, no
/// `die` reaches the server again, and no action is `unknown`. The task halts
/// `tool_unavailable` (REQ-TOOL-009). Read from the store and the server's log.
#[test]
fn req_tool_009_restarts_made_by_the_pin_check_count_and_past_the_limit_the_call_is_not_made() {
    let limit = mimlys_core::limits::LIM_X3_RESTARTS as usize;
    // More steps proposed than the allowance serves. An idempotent read whose
    // worker was lost is tried again once (attempt 2), so the first process and
    // the LIM-X3 restarts serve 1 + LIM-X3 executions in all; the one after them
    // is the attempt that must be refused.
    let (env, log) = dying_env("external_read", limit + 1);
    let scope = env.scope();
    let (code, value) = env.json(&["run", "die", "--scope", scope.to_str().unwrap()]);
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let events = env.events(&id);
    // Server events belong to no task: `metrics` counts them for the store.
    let (_, metrics) = env.json(&["metrics"]);
    assert_eq!(
        metrics["events_by_name"]["server.started"].as_u64(),
        Some(1 + limit as u64),
        "the first start and exactly LIM-X3 restarts: {}",
        metrics["events_by_name"]
    );
    assert_eq!(
        die_calls(&log),
        1 + limit,
        "no `die` reached the server after the limit"
    );
    let finished: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| {
            (
                e["status"].as_str().unwrap().to_owned(),
                e["cause"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    // `action.finished` is recorded per attempt: the executions that reached
    // the server died, the attempts after them (the retry too) were refused.
    let mut expected: Vec<(String, String)> = (0..=limit)
        .map(|_| ("failed".to_owned(), "sandbox_fault".to_owned()))
        .collect();
    expected.extend((0..2).map(|_| ("failed".to_owned(), "tool_unavailable".to_owned())));
    assert_eq!(finished, expected, "{events:?}");
    assert!(
        !events
            .iter()
            .any(|e| e["event"] == "action.finished" && e["status"] == "unknown"),
        "nothing is unknown"
    );
    let task = env.json(&["task", "show", &id]).1;
    assert_eq!(task["task"]["state"], "halted", "{task} (exit {code})");
    assert_eq!(task["task"]["halt_reason"], "tool_unavailable", "{task}");
    let conn = rusqlite::Connection::open(env.data.path().join("data/mimlys.db")).unwrap();
    let last: String = conn
        .query_row(
            "select record from executions order by rowid desc limit 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let last: Value = serde_json::from_str(&last).unwrap();
    assert_eq!(last["termination"], "host_fault", "{last}");
    assert_eq!(last["detail"], "unavailable", "{last}");
    let observation: String = conn
        .query_row(
            "select o.record from observations o join actions a on a.action_id = o.action_id where a.task_id = ?1 order by o.rowid desc limit 1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        observation.contains("restart_limit_reached"),
        "the reason is in the last observation: {observation}"
    );
}

/// REQ-PERS-008, REQ-TOOL-009 (handoff 010e-5, item 1): the same server with
/// `die` declared `external_write`: the death on the first call is `unknown`
/// (the call was sent, the connection was lost) and the task halts. The count is
/// the task's, not the step's: a later start made for the same task, from the
/// same `Servers`, goes on from the count it has, so the allowance is LIM-X3 in
/// all, and the attempt after it is refused as `restart_limit_reached`. A
/// listing for no task never counts.
#[test]
fn req_pers_008_the_restart_count_belongs_to_the_task_and_a_listing_for_no_task_never_counts() {
    use mimlys_core::ids::TaskId;
    let limit = mimlys_core::limits::LIM_X3_RESTARTS;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut config = ServerConfig::new("t", testserver(), vec![]);
        config.env_allow = vec!["PATH".into()];
        let server = Server::new(config);
        let task = TaskId::from_uuid(uuid::Uuid::now_v7());
        let other = TaskId::from_uuid(uuid::Uuid::now_v7());
        let (_, _, hashes) = server.list().await.unwrap();
        assert!(hashes.contains_key("die"));

        // A listing for no task restarts a dead server and counts for nobody:
        // any number of them leave the task its whole allowance.
        for _ in 0..(limit + 2) {
            kill_server(&server).await;
            server.list().await.unwrap();
        }
        let mut deaths = 0;
        // The task's allowance: each death is followed by a restart made for it
        // (the pin check's listing), LIM-X3 times, whichever step it is made in.
        for i in 0..limit {
            kill_server(&server).await;
            deaths += 1;
            let current = server.current_hash(task, "web_search").await;
            assert!(matches!(current, Ok(Some(_))), "{i}: {current:?}");
        }
        assert_eq!(deaths, limit);
        // Another task has its own allowance.
        kill_server(&server).await;
        assert!(server.current_hash(other, "web_search").await.is_ok());
        // The same task again, at once: it is the same count, the allowance is
        // used, and the refusal is not once but every time.
        kill_server(&server).await;
        for _ in 0..3 {
            let refused = server.current_hash(task, "web_search").await;
            assert!(
                matches!(refused, Err(mimlys_mcp::server::ServerError::Died { .. })),
                "{refused:?}"
            );
            let called = server
                .call(
                    task,
                    "engine_status",
                    Default::default(),
                    30_000,
                    &Cancellation::new(),
                )
                .await;
            assert!(
                matches!(called, Err(mimlys_mcp::server::ServerError::Died { .. })),
                "{called:?}"
            );
        }
        // The refusal of one task is not the server's: a listing for no task
        // still starts it.
        assert!(server.list().await.is_ok());
        server.shutdown().await;
    });
}

/// REQ-TOOL-009, REQ-MCP-002 (handoff 010e-5, item 1): the relisting a task's
/// provider makes at a step boundary (`refresh_pins`, after the list changed) is
/// made on the task's behalf: a restart it makes counts against the task, and
/// past the limit it neither starts the server nor unpins the tools for a
/// definition that did not change (the call is refused later, as
/// `restart_limit_reached`).
#[test]
fn req_tool_009_the_relisting_at_a_step_boundary_counts_for_its_task() {
    use mimlys_core::ids::TaskId;
    use mimlys_core::spi::ToolProvider;
    let limit = mimlys_core::limits::LIM_X3_RESTARTS;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut config = ServerConfig::new("t", testserver(), vec![]);
        config.env_allow = vec!["PATH".into()];
        let servers =
            std::sync::Arc::new(mimlys_mcp::tools::Servers::new(vec![Server::new(config)]));
        let server = std::sync::Arc::clone(&servers.all()[0]);
        let task = TaskId::from_uuid(uuid::Uuid::now_v7());
        let provider =
            mimlys_mcp::tools::McpTools::open(task, std::sync::Arc::clone(&servers)).await;
        let pinned = provider.imports();
        assert!(!pinned.is_empty());
        for i in 0..limit {
            kill_server(&server).await;
            server.force_relist();
            let changed = provider.refresh_pins(&pinned);
            assert!(changed.is_empty(), "{i}: {changed:?}");
            assert!(server.is_running().await, "{i}: restarted for the task");
        }
        // The allowance is used: the next relisting does not start the server,
        // and the tools stay pinned.
        kill_server(&server).await;
        server.force_relist();
        let changed = provider.refresh_pins(&pinned);
        assert!(changed.is_empty(), "{changed:?}");
        assert!(!server.is_running().await, "not restarted past the limit");
        assert!(matches!(
            server.current_hash(task, "web_search").await,
            Err(mimlys_mcp::server::ServerError::Died { .. })
        ));
        assert!(
            provider
                .definitions()
                .iter()
                .any(|d| d.name == "t.web_search")
        );
        server.shutdown().await;
    });
}

/// REQ-PERS-008 (handoff 010e-5, item 1): the end-to-end half of the above: with
/// `die` declared `external_write` the first death is `unknown` and the task
/// halts `unknown_outcome` with the action listed; the call reaches the server
/// once, and the model is not asked again.
#[test]
fn req_pers_008_a_death_of_an_external_write_is_unknown_and_halts() {
    let (env, log) = dying_env("external_write", 3);
    let scope = env.scope();
    let (code, value) = env.json(&["run", "die", "--scope", scope.to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let (code, value) = env.json(&["approve", &id]);
    assert_eq!(code, 0, "{value}");
    let task = env.json(&["task", "show", &id]).1;
    assert_eq!(task["task"]["state"], "halted", "{task}");
    assert_eq!(task["task"]["halt_reason"], "unknown_outcome", "{task}");
    let events = env.events(&id);
    let finished: Vec<&str> = events
        .iter()
        .filter(|e| e["event"] == "action.finished")
        .map(|e| e["status"].as_str().unwrap())
        .collect();
    assert_eq!(finished, ["unknown"], "{events:?}");
    assert_eq!(die_calls(&log), 1);
    // One start for each of the two processes (`run`, `approve`), no restart.
    let (_, metrics) = env.json(&["metrics"]);
    assert_eq!(
        metrics["events_by_name"]["server.started"].as_u64(),
        Some(2),
        "{}",
        metrics["events_by_name"]
    );
}

/// REQ-MCP-001, LIM-O16: at runtime exit a server's standard input is closed
/// and the server is given `shutdown_ms` to end by itself (5 000 ms by
/// default, not LIM-O2's 2 000): one that needs 3 s ends on its own and is
/// not killed; one still running after `shutdown_ms` is killed. Both are read
/// from state (a marker the server creates only if it ends by itself), not
/// from a short elapsed time.
#[test]
fn req_mcp_001_a_server_gets_shutdown_ms_to_end_by_itself() {
    assert_eq!(mimlys_core::limits::LIM_O16_SERVER_SHUTDOWN_MS, 5_000);
    assert_eq!(
        ServerConfig::new("t", testserver(), vec![]).shutdown_ms,
        5_000
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let configured = |exit_delay_ms: u64, marker: &std::path::Path, shutdown_ms: i64| {
        let mut config = ServerConfig::new("t", testserver(), vec![]);
        config.env_allow = vec!["PATH".into()];
        config.env_secrets = vec![
            (
                "MIMLYS_TESTSERVER_EXIT_DELAY_MS".into(),
                exit_delay_ms.to_string(),
            ),
            (
                "MIMLYS_TESTSERVER_EXIT_MARKER".into(),
                marker.to_string_lossy().into_owned(),
            ),
        ];
        config.shutdown_ms = shutdown_ms;
        config
    };

    // A graceful shutdown of 4.5 s (longer than the 3 s after which the SDK's own
    // stdio shutdown sends SIGTERM), well inside the grace: it ends by itself,
    // and `shutdown` returns only after it has.
    let graceful = dir.path().join("graceful");
    rt.block_on(async {
        let server = Server::new(configured(4_500, &graceful, 20_000));
        server.list().await.unwrap();
        server.shutdown().await;
    });
    assert!(
        graceful.exists(),
        "a server that needed 4.5 s was killed before it ended by itself"
    );

    // One that outlasts the grace is killed: it never creates its marker,
    // even after the time it would have taken.
    //
    // Decided by the process's state, not by a fixed wait for a marker that may
    // come late: the server beats into a file every 50 ms for as long as it lives
    // (a killed process stops writing), and a file that has stopped growing is a
    // process that is gone. It needs 4 s after its input closes, and the grace is
    // 0.5 s: alive after `shutdown`, it would still be beating.
    let outlasting = dir.path().join("outlasting");
    let beat = dir.path().join("outlasting-heartbeat");
    rt.block_on(async {
        let mut config = configured(4_000, &outlasting, 500);
        config.env_secrets.push((
            "MIMLYS_TESTSERVER_HEARTBEAT".into(),
            beat.to_string_lossy().into_owned(),
        ));
        let server = Server::new(config);
        server.list().await.unwrap();
        server.shutdown().await;
    });
    let len = |p: &std::path::Path| std::fs::metadata(p).map_or(0, |m| m.len());
    assert!(len(&beat) > 0, "the server never beat");
    let started = std::time::Instant::now();
    loop {
        let before = len(&beat);
        std::thread::sleep(std::time::Duration::from_millis(1_000));
        if len(&beat) == before {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "a server that outlasted shutdown_ms was not killed: it still beats"
        );
    }
    assert!(
        !outlasting.exists(),
        "a server that outlasted shutdown_ms was not killed"
    );
}

/// REQ-MCP-001: the servers end together. Two servers that each need 20 s after
/// their input closes end at about the same time, not one after the other: the
/// exit takes the longest grace, not the sum. Judged by state: the two markers
/// are created within 10 s of each other (one after the other they would be
/// 20 s apart, because the second one's input would close only when the first had
/// ended; the hold is far longer than the stall of a slow machine, the rule of
/// handoff 011i), and both are there when `shutdown` returns.
#[test]
fn req_mcp_001_servers_end_together() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let markers = [dir.path().join("first"), dir.path().join("second")];
    rt.block_on(async {
        let servers = mimlys_mcp::tools::Servers::new(
            markers
                .iter()
                .enumerate()
                .map(|(i, marker)| {
                    let mut config = ServerConfig::new(&format!("t{i}"), testserver(), vec![]);
                    config.env_allow = vec!["PATH".into()];
                    config.env_secrets = vec![
                        ("MIMLYS_TESTSERVER_EXIT_DELAY_MS".into(), "20000".into()),
                        (
                            "MIMLYS_TESTSERVER_EXIT_MARKER".into(),
                            marker.to_string_lossy().into_owned(),
                        ),
                    ];
                    config.shutdown_ms = 30_000;
                    Server::new(config)
                })
                .collect(),
        );
        for server in servers.all() {
            server.list().await.unwrap();
        }
        servers.shutdown().await;
    });
    let modified: Vec<std::time::SystemTime> = markers
        .iter()
        .map(|m| {
            std::fs::metadata(m)
                .unwrap_or_else(|_| panic!("{} was killed or never ended", m.display()))
                .modified()
                .unwrap()
        })
        .collect();
    let apart = modified[0]
        .duration_since(modified[1])
        .or_else(|e| Ok::<_, ()>(e.duration()))
        .unwrap();
    assert!(
        apart < std::time::Duration::from_secs(10),
        "the servers ended {apart:?} apart: one after the other"
    );
}

/// REQ-MCP-007 (K-05): a server that is abandoned (it ignored the cancellation
/// and the ping, so the call is `Killed`) is dead, not merely forgotten, and
/// so is every process it started (the kill reaches the group): both stop
/// writing their heartbeat. The check is made on state, files that no longer
/// grow.
#[test]
fn req_mcp_007_a_killed_server_is_gone() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let beat = dir.path().join("heartbeat");
    let grand = dir.path().join("grandchild");
    rt.block_on(async {
        let mut config = ServerConfig::new("t", testserver(), vec![]);
        config.env_allow = vec!["PATH".into()];
        config.env_secrets = vec![
            ("MIMLYS_TESTSERVER_IGNORE_CANCEL".into(), "1".into()),
            (
                "MIMLYS_TESTSERVER_HEARTBEAT".into(),
                beat.to_string_lossy().into_owned(),
            ),
            (
                "MIMLYS_TESTSERVER_GRANDCHILD".into(),
                grand.to_string_lossy().into_owned(),
            ),
        ];
        let server = Server::new(config);
        let task = mimlys_core::ids::TaskId::from_uuid(uuid::Uuid::now_v7());
        let cancel = Cancellation::new();
        cancel.cancel();
        let outcome = server
            .call(
                task,
                "slow",
                json!({"ms": 60000}).as_object().unwrap().clone(),
                120_000,
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(outcome, CallOutcome::Killed);
        let len = |p: &std::path::Path| std::fs::metadata(p).map_or(0, |m| m.len());
        assert!(len(&beat) > 0, "the server never beat");
        assert!(len(&grand) > 0, "the server's child never beat");
        // Wait until the files have stopped growing: a second without a byte is
        // twenty beats missed (they come every 50 ms), and a machine that is slow
        // to deliver the kill gets up to 60 s for it. A process that is still
        // alive beats for ever and never lets the files settle.
        let started = std::time::Instant::now();
        loop {
            let before = (len(&beat), len(&grand));
            tokio::time::sleep(std::time::Duration::from_millis(1_000)).await;
            let after = (len(&beat), len(&grand));
            if before == after {
                break;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(60),
                "{}",
                if before.0 != after.0 {
                    "the killed server is still running"
                } else {
                    "a process the killed server started is still running"
                }
            );
        }
    });
}

/// REQ-MCP-001, LIM-O16: `check` warns when a server's `shutdown_ms` is below
/// 1 000 ms (its group is killed before most servers finish ending), names the
/// server and the value, and says nothing at 1 000 ms or when it is unset.
#[test]
fn req_mcp_001_check_warns_of_a_short_shutdown_ms() {
    let short = Env::new(&[], "shutdown_ms = 900", &[]);
    let (code, stdout, _) = short.run(&["check"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("warning: servers.rypsok.shutdown_ms is 900"),
        "{stdout}"
    );
    let (_, check) = short.json(&["check"]);
    assert_eq!(
        check["short_shutdown_servers"],
        json!([{"server": "rypsok", "shutdown_ms": 900}])
    );
    for ok in ["shutdown_ms = 1000", ""] {
        let env = Env::new(&[], ok, &[]);
        let (code, stdout, _) = env.run(&["check"]);
        assert_eq!(code, 0, "{stdout}");
        assert!(!stdout.contains("shutdown_ms is"), "{ok}: {stdout}");
    }
}

/// RFC 009 §4 and the configuration surface: a declaration admits the tool
/// to the ceiling and `check` says by what; an undeclared tool in the
/// ceiling is warned about; invalid keys are refused by name.
#[test]
fn m5_configuration_and_check() {
    let env = Env::new(&[], "", &[]);
    let (code, stdout, _) = env.run(&["check"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("ceiling: rypsok.web_search"), "{stdout}");
    assert!(
        stdout.contains("declared by [servers.rypsok.tools.web_search]"),
        "{stdout}"
    );
    assert!(
        stdout.contains("warning: rypsok.engine_status is in policy.ceiling.tools without"),
        "{stdout}"
    );
    let (_, check) = env.json(&["check"]);
    assert!(
        check["external_tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["tool"] == "rypsok.web_fetch")
    );
    assert_eq!(
        check["undeclared_external_tools"],
        json!(["rypsok.die", "rypsok.engine_status"])
    );

    for (bad, key) in [
        ("[servers.Bad]\ncommand = \"x\"\n", "servers.Bad"),
        ("[servers.s]\ncommand = \"\"\n", "servers.s.command"),
        (
            "[servers.s]\ncommand = \"x\"\nstartup_ms = 10\n",
            "servers.s.startup_ms",
        ),
        (
            "[servers.s]\ncommand = \"x\"\nshutdown_ms = 499\n",
            "servers.s.shutdown_ms",
        ),
        (
            "[servers.s]\ncommand = \"x\"\nshutdown_ms = 30001\n",
            "servers.s.shutdown_ms",
        ),
        (
            "[servers.s]\ncommand = \"x\"\n[servers.s.tools.t]\neffect = \"workspace_write\"\n",
            "external_read",
        ),
        (
            "[servers.s]\ncommand = \"x\"\n[servers.s.tools.t]\neffect = \"external_read\"\ntimeout_ms = 1\n",
            "servers.s.tools.t.timeout_ms",
        ),
    ] {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n{bad}",
                data.path().join("d").to_string_lossy()
            ),
        )
        .unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_mimlys"))
            .arg("--config")
            .arg(&config)
            .arg("check")
            .env("MIMLYS_CONFIG", "")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{bad}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(key), "{bad}: {stderr}");
    }
}

/// External design §23, M5, against a real `rypsok serve` when
/// `MIMLYS_RYPSOK_BIN` names the binary. rypsok is started with no
/// configuration file, so its `web_search` reaches the real Wikipedia over
/// the internet; no fixture server is started. The test checks only that the
/// task reaches `completed`: the scripted backend completes it whatever the
/// tool answered, so an unavailable search (no network) passes too. It shows
/// that the real binary starts, is imported and pinned, and is called;
/// it does not check an answer.
#[test]
fn m5_real_rypsok() {
    let Some(bin) = std::env::var_os("MIMLYS_RYPSOK_BIN").filter(|v| !v.is_empty()) else {
        eprintln!("skipped: set MIMLYS_RYPSOK_BIN to run against a real rypsok");
        return;
    };
    let data = tempfile::tempdir().unwrap();
    let script = data.path().join("script.json");
    std::fs::write(
        &script,
        serde_json::to_string(&[
            act(
                "rypsok.web_search",
                json!({"query": "rust programming language", "max_results": 3}),
            ),
            complete(),
        ])
        .unwrap(),
    )
    .unwrap();
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
tools = [{builtin}, "rypsok.web_search", "rypsok.web_fetch"]
[servers.rypsok]
command = {bin:?}
args = ["serve"]
env_allow = ["PATH"]
[servers.rypsok.tools.web_search]
effect = "external_read"
idempotent = true
[servers.rypsok.tools.web_fetch]
effect = "external_read"
idempotent = true
destination_arguments = ["url"]
"#,
            data = data.path().join("d").to_string_lossy(),
            script = script.to_string_lossy(),
            builtin = BUILTIN,
            bin = PathBuf::from(&bin).to_string_lossy(),
        ),
    )
    .unwrap();
    let scope = data.path().join("scope");
    std::fs::create_dir_all(&scope).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
    cmd.args([
        "--config",
        config.to_str().unwrap(),
        "--json",
        "run",
        "search",
        "--scope",
        scope.to_str().unwrap(),
    ])
    .env("MIMLYS_CONFIG", "")
    .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
    let out = run_bounded(&mut cmd, data.path());
    let stdout = out.stdout;
    eprintln!("{stdout}\n{}", out.stderr);
    let value: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(value["task"]["state"], "completed");
}

/// The effective deadline of each action of a task, by tool, from the store.
fn deadlines(db: &std::path::Path, task_id: &str) -> std::collections::BTreeMap<String, i64> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("select record from actions where task_id = ?1")
        .unwrap();
    stmt.query_map([task_id], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|text| {
            let record: Value = serde_json::from_str(&text.unwrap()).unwrap();
            (
                record["tool"].as_str().unwrap().to_owned(),
                record["deadline_ms"].as_i64().unwrap(),
            )
        })
        .collect()
}

/// K-03 (external design §11, limits LIM-O5): the deadline of an external
/// tool is its declaration's `timeout_ms`, and LIM-O5 when none is given,
/// for a declared tool without a `timeout_ms` and for an undeclared one.
/// This test claims only the default (no `max_call_ms` is configured); the
/// floor that `max_call_ms` sets is `req_mcp_008_deadline_is_never_below_max_call_ms`.
#[test]
fn k_03_external_deadline_defaults_to_lim_o5() {
    let env = Env::new(
        &[
            act("rypsok.web_search", json!({"query": "rust"})),
            act("rypsok.echo_env", json!({})),
            act("rypsok.engine_status", json!({})),
            complete(),
        ],
        "",
        &["rypsok.engine_status"],
    );
    let scope = env.scope();
    let (code, value) = env.json(&[
        "run",
        "deadlines",
        "--scope",
        scope.to_str().unwrap(),
        "--grant",
        "network:example.org",
    ]);
    assert_eq!(code, 0, "{value}");
    let id = value["task"]["task_id"].as_str().unwrap();
    let got = deadlines(&env.data.path().join("data/mimlys.db"), id);
    assert_eq!(got["rypsok.web_search"], 10_000, "the declared timeout_ms");
    assert_eq!(
        got["rypsok.echo_env"],
        mimlys_core::limits::LIM_O5_EXTERNAL_DEADLINE_MS,
        "declared without a timeout_ms"
    );
    assert_eq!(
        got["rypsok.engine_status"],
        mimlys_core::limits::LIM_O5_EXTERNAL_DEADLINE_MS,
        "undeclared"
    );
}

/// REQ-MCP-008 (external design §11): a server's `max_call_ms` is a floor for
/// every deadline of its tools, a declared tool's (`web_search`, declared at
/// 10 000 ms; `echo_env`, declared without a `timeout_ms`) and an undeclared
/// tool's (`engine_status`, LIM-O5 by default). A declaration above the floor
/// keeps its own value (`web_fetch`, 15 000 ms, under a 12 000 ms floor
/// in the second run).
#[test]
fn req_mcp_008_deadline_is_never_below_max_call_ms() {
    let run = |max_call_ms: i64| {
        let env = Env::new(
            &[
                act("rypsok.web_search", json!({"query": "rust"})),
                act("rypsok.echo_env", json!({})),
                act(
                    "rypsok.web_fetch",
                    json!({"url": "https://example.org/article"}),
                ),
                act("rypsok.engine_status", json!({})),
                complete(),
            ],
            &format!("max_call_ms = {max_call_ms}"),
            &["rypsok.engine_status"],
        );
        let scope = env.scope();
        let (code, value) = env.json(&[
            "run",
            "floor",
            "--scope",
            scope.to_str().unwrap(),
            "--grant",
            "network:example.org",
        ]);
        assert_eq!(code, 0, "{value}");
        let id = value["task"]["task_id"].as_str().unwrap();
        deadlines(&env.data.path().join("data/mimlys.db"), id)
    };
    let high = run(120_000);
    for tool in [
        "rypsok.web_search",
        "rypsok.echo_env",
        "rypsok.engine_status",
        "rypsok.web_fetch",
    ] {
        assert_eq!(
            high.get(tool),
            Some(&120_000),
            "{tool}: raised to max_call_ms: {high:?}"
        );
    }
    let low = run(12_000);
    assert_eq!(low["rypsok.web_search"], 12_000, "declared 10 000, raised");
    assert_eq!(low["rypsok.web_fetch"], 15_000, "declared above the floor");
    assert_eq!(
        low["rypsok.echo_env"],
        mimlys_core::limits::LIM_O5_EXTERNAL_DEADLINE_MS,
        "the default is above the floor"
    );
    assert_eq!(
        low["rypsok.engine_status"],
        mimlys_core::limits::LIM_O5_EXTERNAL_DEADLINE_MS,
        "undeclared: the default is above the floor"
    );
}

// Handoff 009f: can `rmcp`'s client cache of `tools/list` hand mimlys an old
// list? The stand-in sends `ttlMs` (the hint that makes the SDK keep a
// response), can be made to fail a later `tools/list`, and logs every request
// that reaches it.

/// A server on the stand-in with the listing knobs; `log` collects one line per
/// `tools/list` request that reached the server.
fn listing_server(ttl_ms: u64, fail_after: Option<u32>, log: &std::path::Path) -> Server {
    let mut config = ServerConfig::new("t", testserver(), vec![]);
    config.env_allow = vec!["PATH".into()];
    config.env_secrets = vec![
        ("MIMLYS_TESTSERVER_LIST_TTL_MS".into(), ttl_ms.to_string()),
        (
            "MIMLYS_TESTSERVER_LIST_LOG".into(),
            log.to_string_lossy().into_owned(),
        ),
    ];
    if let Some(n) = fail_after {
        config
            .env_secrets
            .push(("MIMLYS_TESTSERVER_FAIL_LIST_AFTER".into(), n.to_string()));
    }
    Server::new(config)
}

fn list_requests_seen(log: &std::path::Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |t| t.lines().count())
}

/// Asks the stand-in to change a definition (it then sends
/// `notifications/tools/list_changed`) and waits until mimlys has seen it.
async fn change_and_wait(server: &Server) {
    let task = mimlys_core::ids::TaskId::from_uuid(uuid::Uuid::now_v7());
    let outcome = server
        .call(
            task,
            "change_definition",
            Default::default(),
            30_000,
            &Cancellation::new(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, CallOutcome::Result { .. }), "{outcome:?}");
    // Up to 60 s: the list is dirty once the notification has been read.
    for _ in 0..2000 {
        if server.is_dirty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    panic!("the list_changed notification never reached mimlys");
}

/// REQ-MCP-002, THR-25 (handoff 009f): a server that listed once, sent `ttlMs`
/// (so that the SDK's client cache holds its first answer for ten minutes),
/// announced a changed list and then cannot list: mimlys gets `unavailable`
/// with the reason, never the SDK's cached list, and the relist did reach the
/// server. The SDK drops its tool-list cache when the notification arrives,
/// before mimlys's handler marks the list dirty, so nothing is left to serve.
#[test]
fn req_mcp_002_a_failed_listing_is_unavailable_and_never_the_sdks_cached_list() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("lists.log");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let server = listing_server(600_000, Some(1), &log);
        let (_, first, first_hashes) = server.list().await.unwrap();
        assert_eq!(list_requests_seen(&log), 1);
        assert!(first.iter().any(|t| t.name == "web_search"));

        change_and_wait(&server).await;
        for attempt in 1..=3 {
            let err = server.list().await.unwrap_err();
            let mimlys_mcp::server::ServerError::Unavailable { reason, .. } = &err else {
                panic!("not unavailable: {err:?}");
            };
            assert!(reason.starts_with("list:"), "{reason}");
            assert_eq!(
                list_requests_seen(&log),
                1 + attempt,
                "attempt {attempt}: the relist reached the server, it was not answered by a cache"
            );
            assert!(
                server.is_dirty(),
                "the list stays dirty until a listing succeeds"
            );
        }
        // What mimlys last saw is untouched and is not offered as a fresh listing.
        assert_eq!(server.last_list().unwrap().2, first_hashes);
        // A server that cannot list is the error, not a tool that is gone: the
        // pin check gives the call `tool_unavailable`, not `definition_changed`.
        let task = mimlys_core::ids::TaskId::from_uuid(uuid::Uuid::now_v7());
        assert!(matches!(
            server.current_hash(task, "web_fetch").await,
            Err(mimlys_mcp::server::ServerError::Unavailable { .. })
        ));
        server.shutdown().await;
    });
}

/// REQ-MCP-002 (handoff 009f): the same server without the failure: after the
/// notification the relist reaches the server and brings the new definition,
/// although the SDK held the first answer for ten minutes.
#[test]
fn req_mcp_002_a_relist_after_list_changed_reaches_the_server_despite_ttl() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("lists.log");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let server = listing_server(600_000, None, &log);
        let (_, _, before) = server.list().await.unwrap();
        change_and_wait(&server).await;
        let (_, _, after) = server.list().await.unwrap();
        assert_eq!(
            list_requests_seen(&log),
            2,
            "the second listing reached the server"
        );
        assert_ne!(before, after, "and brought the changed definition");
        assert!(!server.is_dirty());
        server.shutdown().await;
    });
}

/// REQ-MCP-002, THR-25 (handoff 009f, the edge): the SDK's client cache is off
/// (`Connection::start`), so even a relist **without** the server's
/// notification, which no product code makes, reaches the server. With `ttlMs`
/// above zero the second listing is not answered from a cache, and a server that
/// then fails is `unavailable`: the expired list the SDK would have served on
/// error never reaches mimlys. With `ttlMs: 0` (rypsok 0.10.0, and the SDK's
/// assumption when the hint is absent) the result is the same.
#[test]
fn req_mcp_002_a_relist_without_the_notification_still_reaches_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        for ttl in [600_000_u64, 0] {
            let log = dir.path().join(format!("lists-{ttl}.log"));
            let server = listing_server(ttl, None, &log);
            server.list().await.unwrap();
            server.force_relist();
            server.list().await.unwrap();
            assert_eq!(
                list_requests_seen(&log),
                2,
                "ttlMs {ttl}: the second listing was not answered from a cache"
            );
            server.shutdown().await;
        }
        // What the SDK's stale-on-error branch would have done: `ttlMs` 50, a listing that
        // fails after the entry expired. mimlys gets `unavailable`, not the expired list.
        let log = dir.path().join("lists-stale.log");
        let server = listing_server(50, Some(1), &log);
        server.list().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        server.force_relist();
        let err = server.list().await.unwrap_err();
        let mimlys_mcp::server::ServerError::Unavailable { reason, .. } = &err else {
            panic!("not unavailable: {err:?}");
        };
        assert!(reason.starts_with("list:"), "{reason}");
        assert_eq!(
            list_requests_seen(&log),
            2,
            "the server was asked and refused"
        );
        assert!(server.is_dirty());
        server.shutdown().await;
    });
}

/// Makes the stand-in exit (its `die` tool) and waits until mimlys sees the
/// transport gone.
async fn kill_server(server: &Server) {
    let task = mimlys_core::ids::TaskId::from_uuid(uuid::Uuid::now_v7());
    let _ = server
        .call(
            task,
            "die",
            Default::default(),
            30_000,
            &Cancellation::new(),
        )
        .await;
    // Up to 60 s: the transport is gone once the process has ended.
    for _ in 0..2000 {
        if !server.is_running().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    panic!("the server did not stop");
}

/// REQ-MCP-001, REQ-MCP-002, THR-25 (handoff 009f, rypsok's facts): a listing
/// cached before a server restart is never served after it. The SDK's client
/// object, and so its cache, is made by `Connection::start` for each start of a
/// server process and dropped with it; nothing carries over. The stand-in sends
/// `ttlMs` of ten minutes, so a client that outlived the process would answer the
/// second listing from its cache, and a failing restarted process would be
/// answered with the old list. Neither happens: the new process is asked, and
/// its answer, the new definitions or its refusal, is what mimlys gets.
#[test]
fn req_mcp_002_a_listing_cached_before_a_restart_is_not_served_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("lists.log");
    let changed = dir.path().join("changed");
    let failing = dir.path().join("failing");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut config = ServerConfig::new("t", testserver(), vec![]);
        config.env_allow = vec!["PATH".into()];
        config.env_secrets = vec![
            ("MIMLYS_TESTSERVER_LIST_TTL_MS".into(), "600000".into()),
            (
                "MIMLYS_TESTSERVER_LIST_LOG".into(),
                log.to_string_lossy().into_owned(),
            ),
            (
                "MIMLYS_TESTSERVER_CHANGED_FILE".into(),
                changed.to_string_lossy().into_owned(),
            ),
            (
                "MIMLYS_TESTSERVER_FAIL_LIST_FILE".into(),
                failing.to_string_lossy().into_owned(),
            ),
        ];
        let server = Server::new(config);
        let (_, _, before) = server.list().await.unwrap();
        assert_eq!(list_requests_seen(&log), 1);

        // The server dies and comes back with other definitions (an upgrade, or
        // another configuration): the next listing is the new one.
        kill_server(&server).await;
        std::fs::write(&changed, b"upgraded").unwrap();
        let (_, _, after) = server.list().await.unwrap();
        assert_ne!(
            before, after,
            "the restarted server's definitions, not the cached ones"
        );
        assert_eq!(list_requests_seen(&log), 2, "the new process was asked");

        // It dies again and the next process cannot list: unavailable, never the old list.
        kill_server(&server).await;
        std::fs::write(&failing, b"no").unwrap();
        let err = server.list().await.unwrap_err();
        let mimlys_mcp::server::ServerError::Unavailable { reason, .. } = &err else {
            panic!("not unavailable: {err:?}");
        };
        assert!(reason.starts_with("list:"), "{reason}");
        assert_eq!(
            list_requests_seen(&log),
            3,
            "the third process was asked and refused"
        );
        std::fs::remove_file(&failing).unwrap();
        server.shutdown().await;
    });
}
