//! The M4 demonstration (external design §23; RFCs 007 and 008): a model
//! repairs a scripted defect in a JavaScript workload within LIM-T1 steps
//! through the OpenAI-compatible adapter, with segments 1–4 byte-stable
//! across the run. The protocol is proven against the mock server in every
//! test run; the real-engine run is executed when `MIMLYS_M4_BACKEND_URL`
//! and `MIMLYS_M4_MODEL` name a server, and its transcript goes into the
//! report (`docs/src/maintainers/m4-report.md`).
//!
//! The QuickJS build comes from `MIMLYS_QJS_WASM` when set, else from the
//! pinned artifact through `mimlys interpreters install` (network).

use mimlys_model::mock::{MockConfig, MockServer, Reply, UsageStyle};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

mod common;
use common::run_bounded;

/// The QuickJS module, fetched once per test run.
fn qjs() -> PathBuf {
    static MODULE: OnceLock<PathBuf> = OnceLock::new();
    MODULE
        .get_or_init(|| {
            if let Some(p) = std::env::var_os("MIMLYS_QJS_WASM") {
                return PathBuf::from(p);
            }
            let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/interpreters");
            let data = cache.join("data");
            let pinned = mimlys_exec::interpreters::pinned("javascript").unwrap();
            let path = mimlys_exec::interpreters::default_module_path(&data, pinned.sha256);
            if !path.is_file() {
                std::fs::create_dir_all(&data).unwrap();
                let config = cache.join("config.toml");
                std::fs::write(
                    &config,
                    format!(
                        "config_version = 1\n[runtime]\ndata_dir = {:?}\n",
                        data.to_string_lossy()
                    ),
                )
                .unwrap();
                let out = Command::new(env!("CARGO_BIN_EXE_mimlys"))
                    .args([
                        "--config",
                        config.to_str().unwrap(),
                        "interpreters",
                        "install",
                        "javascript",
                    ])
                    .env("MIMLYS_CONFIG", "")
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            path
        })
        .clone()
}

const SUM_JS: &str = "// Sums the integers from 1 to n inclusive.\nexport function sum(n) {\n  let total = 0;\n  for (let i = 1; i < n; i++) {\n    total += i;\n  }\n  return total;\n}\n";
const FIXED_SUM_JS: &str = "// Sums the integers from 1 to n inclusive.\nexport function sum(n) {\n  let total = 0;\n  for (let i = 1; i <= n; i++) {\n    total += i;\n  }\n  return total;\n}\n";
const TEST_JS: &str = "import { sum } from '/workspace/tools/sum.js';\nconst cases = [[1, 1], [4, 10], [10, 55]];\nconst failures = [];\nfor (const [n, want] of cases) {\n  const got = sum(n);\n  if (got !== want) { failures.push(`sum(${n}) = ${got}, want ${want}`); }\n}\nif (failures.length > 0) { throw new Error('FAIL ' + failures.join('; ')); }\nconsole.log('ok');\n";

/// The project: a module with an off-by-one and a test that prints `ok`
/// or exits 1.
fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("tools")).unwrap();
    std::fs::create_dir_all(dir.path().join("tests")).unwrap();
    std::fs::write(dir.path().join("tools/sum.js"), SUM_JS).unwrap();
    std::fs::write(dir.path().join("tests/sum.test.js"), TEST_JS).unwrap();
    dir
}

/// The goal and the completion check shared by both runs. `run_script`
/// takes the source text, so the check carries the test file's content;
/// it imports the module from the workspace.
const GOAL: &str = "tests/sum.test.js throws when run with run_script: fix tools/sum.js so that the test prints ok. Read both files first; run the test with run_script (language javascript, the test file's content as source).";

fn run_test() -> Value {
    json!({"language": "javascript", "source": TEST_JS, "mode": "read_only"})
}

fn completion_check() -> String {
    json!({"tool": "run_script", "arguments": run_test()}).to_string()
}

struct Env {
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new(backend: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\n{backend}\n[interpreters.javascript]\nmodule = {:?}\n",
                data.path().join("data").to_string_lossy(),
                qjs().to_string_lossy(),
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

/// The scripted repair: read the test, read the module, run the test (it
/// fails), replace the comparison, run the test (ok), complete.
fn repair_script() -> Vec<Reply> {
    let act = |tool: &str, args: Value| {
        Reply::Decision(json!({"decision": "act", "tool": tool, "arguments": args}))
    };
    vec![
        Reply::Decision(
            json!({"decision": "plan", "steps": ["read the test and the module", "run the test", "fix the loop bound", "run the test again"]}),
        ),
        act("read_file", json!({"path": "tests/sum.test.js"})),
        act("read_file", json!({"path": "tools/sum.js"})),
        act("run_script", run_test()),
        act(
            "replace_in_file",
            json!({"path": "tools/sum.js", "find": "i < n", "replace": "i <= n"}),
        ),
        act("run_script", run_test()),
        Reply::Decision(
            json!({"decision": "complete", "summary": "the loop now includes n; the test prints ok"}),
        ),
    ]
}

/// External design §23, M4, against the mock server: the adapter carries
/// the loop end to end (`completed{verified}` within LIM-T1 steps), the
/// probe is visible in `status` and `runtime.started`, the task records
/// its backend, the prefix of segments 1–4 is byte-stable across the run,
/// and the usage the server reports reaches the metrics.
#[test]
fn m4_mock_engine_repairs_the_defect() {
    let server = MockServer::start(
        MockConfig {
            refuse_formats: vec!["json_schema".into()],
            ..MockConfig::default()
        },
        repair_script(),
    );
    let env = Env::new(&format!(
        "kind = \"openai_compatible\"\nurl = {:?}\nmodel = \"test-model\"\nstructured_output = \"auto\"",
        server.url()
    ));
    let scope = scope();

    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["kind"], "openai_compatible");
    assert_eq!(status["backend"]["model"], "test-model");
    assert_eq!(status["backend"]["locality"], "local");
    assert_eq!(status["backend"]["reachable"], true);
    assert_eq!(status["backend"]["model_listed"], true);
    assert_eq!(status["backend"]["structured_output"], "json_object");
    assert_eq!(status["backend"]["fell_back"], true);

    let (code, value) = env.json(&[
        "run",
        GOAL,
        "--scope",
        scope.path().to_str().unwrap(),
        "--completion-check",
        &completion_check(),
    ]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    assert_eq!(value["task"]["completion"], "verified", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    assert_eq!(
        std::fs::read_to_string(env.tree(&id).join("tools/sum.js")).unwrap(),
        FIXED_SUM_JS
    );
    assert_eq!(
        std::fs::read_to_string(scope.path().join("tools/sum.js")).unwrap(),
        SUM_JS,
        "the project is untouched until promotion"
    );

    // Every decision request carried json_object; segments 1–4 are byte-stable.
    let requests = server.decision_requests();
    assert!(requests.len() >= 7, "{}", requests.len());
    let first = &requests[0].body["messages"];
    for (k, r) in requests.iter().enumerate() {
        assert_eq!(r.body["response_format"]["type"], "json_object");
        assert_eq!(
            r.body["messages"][0], first[0],
            "segments 1–3 changed at request {k}"
        );
        assert_eq!(
            r.body["messages"][1], first[1],
            "segment 4 changed at request {k}"
        );
        assert_eq!(r.body["max_tokens"], 4096);
        assert_eq!(r.body["stream"], false);
    }

    let (_, task) = env.json(&["task", "show", &id]);
    assert_eq!(
        task["task"]["versions"]["backend"]["kind"],
        "openai_compatible"
    );
    assert_eq!(task["task"]["versions"]["backend"]["model"], "test-model");
    let (_, events) = env.json(&["events", &id]);
    let names: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["record"]["event"].as_str())
        .collect();
    assert!(names.contains(&"workspace.snapshot"), "{names:?}");
    assert!(!names.contains(&"decision.rejected"), "{names:?}");
    let (_, metrics) = env.json(&["metrics", "--task", &id]);
    let c = &metrics["context"];
    eprintln!(
        "m4 mock run: {} decision requests; context {}",
        requests.len(),
        c
    );
    for (k, r) in requests.iter().enumerate() {
        let chars: usize = r.body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap_or("").chars().count())
            .sum();
        eprintln!(
            "  request {k}: {} messages, {chars} chars",
            r.body["messages"].as_array().unwrap().len()
        );
    }
    assert!(c["measured_steps"].as_u64().unwrap() >= 7);
    assert!(c["prompt_tokens"].as_u64().unwrap() > 0, "{c}");
    assert!(c["cached_tokens"].as_u64().unwrap() > 0, "{c}");
    assert!(c["prefix_ratio"].as_f64().unwrap() > 0.5, "{c}");
    assert!(c["chars_per_prompt_token"].as_f64().unwrap() > 1.0, "{c}");
    assert_eq!(c["over_budget_steps"], 0);
    assert_eq!(
        metrics["events_by_name"]["runtime.started"],
        Value::Null,
        "runtime.started has no task"
    );
    let (_, all) = env.json(&["metrics"]);
    assert!(all["events_by_name"]["runtime.started"].as_u64().unwrap() >= 2);
}

/// RFC 008 §2: `auto` that ends at plain text is announced once by `run`
/// and shown by `status`; `schema` against a refusing server is a startup
/// error naming the fix; an unreachable server halts the first step
/// `backend_unavailable` instead of refusing the command.
#[test]
fn m4_structured_output_is_visible() {
    let server = MockServer::start(
        MockConfig {
            refuse_formats: vec!["json_schema".into(), "json_object".into()],
            usage: UsageStyle::None,
            ..MockConfig::default()
        },
        vec![Reply::Decision(
            json!({"decision": "complete", "summary": "demo"}),
        )],
    );
    let env = Env::new(&format!(
        "kind = \"openai_compatible\"\nurl = {:?}\nmodel = \"test-model\"",
        server.url()
    ));
    let scope = scope();
    let (code, stdout, _) = env.run(&["run", GOAL, "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("structured output: none"), "{stdout}");
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["structured_output"], "none");
    assert_eq!(status["backend"]["fell_back"], true);

    let env = Env::new(&format!(
        "kind = \"openai_compatible\"\nurl = {:?}\nmodel = \"test-model\"\nstructured_output = \"schema\"",
        server.url()
    ));
    let (code, _, stderr) = env.run(&["status"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("refuses json_schema"), "{stderr}");
    assert!(stderr.contains("structured_output"), "{stderr}");

    let env =
        Env::new("kind = \"openai_compatible\"\nurl = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"");
    let (code, status) = env.json(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(status["backend"]["reachable"], false);
    assert_eq!(status["backend"]["detail"], "transport");
    let (code, value) = env.json(&["run", GOAL, "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "halted", "{value}");
    assert_eq!(
        value["task"]["halt_reason"], "backend_unavailable",
        "{value}"
    );
}

/// External design §23, M4, against a real local engine: run when
/// `MIMLYS_M4_BACKEND_URL` and `MIMLYS_M4_MODEL` are set (llama-server or
/// Ollama). Prints the transcript and the measures for the report.
#[test]
fn m4_real_engine_repairs_the_defect() {
    let (Some(url), Some(model)) = (
        std::env::var("MIMLYS_M4_BACKEND_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        std::env::var("MIMLYS_M4_MODEL")
            .ok()
            .filter(|s| !s.is_empty()),
    ) else {
        eprintln!(
            "skipped: set MIMLYS_M4_BACKEND_URL and MIMLYS_M4_MODEL to run against a local engine"
        );
        return;
    };
    let env = Env::new(&format!(
        "kind = \"openai_compatible\"\nurl = {url:?}\nmodel = {model:?}\nstructured_output = \"auto\"\ntimeout_ms = 600000"
    ));
    let scope = scope();
    let (_, status) = env.json(&["status"]);
    eprintln!(
        "status: {}",
        serde_json::to_string_pretty(&status["backend"]).unwrap()
    );
    assert_eq!(status["backend"]["reachable"], true, "{status}");
    let (code, value) = env.json(&[
        "run",
        GOAL,
        "--scope",
        scope.path().to_str().unwrap(),
        "--completion-check",
        &completion_check(),
    ]);
    let id = value["task"]["task_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let (_, events) = env.json(&["events", &id]);
    eprintln!("events: {}", serde_json::to_string_pretty(&events).unwrap());
    let (_, metrics) = env.json(&["metrics", "--task", &id]);
    eprintln!(
        "metrics: {}",
        serde_json::to_string_pretty(&metrics["context"]).unwrap()
    );
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "completed", "{value}");
    assert_eq!(value["task"]["completion"], "verified", "{value}");
    assert!(
        std::fs::read_to_string(env.tree(&id).join("tools/sum.js"))
            .unwrap()
            .contains("i <= n")
    );
}
