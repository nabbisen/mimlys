//! A model backend's redirect is never followed (REQ-SEC-010; RFC 015, Amendment 1; external
//! design §20; handoff 015a-5, item 1), through the binary against two mock servers: the first
//! answers every request with `307` to the second, which must receive nothing.
//!
//! A task whose backend redirects is retried as LIM-M3 says and halts `backend_unavailable`, and
//! `status` shows the backend as unreachable on its own line, naming the host the redirect pointed
//! to (never the URL).
#![cfg(feature = "test-bins")]

use mimlys_core::limits::LIM_M3_BACKEND_RETRIES;
use mimlys_model::mock::{MockConfig, MockServer};
use serde_json::Value;
use std::path::PathBuf;

mod common;
use common::run_bounded;

struct Rig {
    dir: tempfile::TempDir,
    config: PathBuf,
    scope: PathBuf,
}

impl Rig {
    /// A configuration with the backend `default` on `first`'s URL.
    fn new(first: &MockServer) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let scope = dir.path().join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        std::fs::write(scope.join("a.txt"), "a\n").unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                r#"config_version = 2
[runtime]
data_dir = {data:?}
[backends.default]
kind = "openai_compatible"
url = {url:?}
model = "test-model"
structured_output = "none"
decision_transport = "envelope"
"#,
                data = dir.path().join("data").to_string_lossy(),
                url = first.url(),
            ),
        )
        .unwrap();
        Self { dir, config, scope }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = common::mimlys();
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.dir.path());
        (out.code(), out.stdout.clone(), out.stderr.clone())
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (_, stdout, stderr) = self.run(&all);
        serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"))
    }
}

fn servers() -> (MockServer, MockServer) {
    let second = MockServer::start(MockConfig::default(), vec![]);
    // `localhost` names the host the redirect points to (the first server is `127.0.0.1`), and
    // reaches the second server if a client followed.
    let base = second
        .url()
        .trim_end_matches("/v1")
        .replace("127.0.0.1", "localhost");
    let first = MockServer::start(
        MockConfig {
            redirect_to: Some(base),
            ..MockConfig::default()
        },
        vec![],
    );
    (first, second)
}

/// REQ-SEC-010, REQ-MDL-003: a task whose backend answers every request with a redirect is retried
/// as LIM-M3 says (the same request, again), halts `backend_unavailable`, and the second server
/// receives nothing: not the decision request, not the probe's listing. The task is made with
/// `--hold` (which contacts no backend) and started by `resume`: a creation that starts the task is
/// refused by the backend that does not answer (REQ-MDL-010, external design section 20), and
/// nothing is created.
#[test]
fn req_sec_010_a_task_whose_backend_redirects_halts_backend_unavailable() {
    let (first, second) = servers();
    let rig = Rig::new(&first);
    let scope = rig.scope.to_str().unwrap();
    // A creation that starts the task is refused, naming the host the redirect points to.
    let (code, _, stderr) = rig.run(&["run", "go", "--scope", scope]);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains("does not answer") && stderr.contains("localhost"),
        "{stderr}"
    );
    assert!(rig.json(&["task", "list"]).as_array().unwrap().is_empty());
    let made = rig.json(&["task", "new", "go", "--hold", "--scope", scope]);
    let id = made["task"]["task_id"].as_str().unwrap().to_owned();
    let value = rig.json(&["task", "resume", &id]);
    assert_eq!(value["task"]["state"], "halted", "{value}");
    assert_eq!(
        value["task"]["halt_reason"], "backend_unavailable",
        "{value}"
    );
    assert_eq!(
        first.decision_requests().len(),
        1 + LIM_M3_BACKEND_RETRIES as usize,
        "the request was repeated as LIM-M3 says, and no more"
    );
    assert!(
        second.requests().is_empty(),
        "the redirect was followed: {:?}",
        second.requests()
    );
}

/// REQ-SEC-010, REQ-MDL-010: `status` says on the backend's own line that it is unreachable and
/// names the host the redirect points to, not the URL; in JSON the detail says the same; the
/// second server receives nothing.
#[test]
fn req_sec_010_status_names_the_host_of_a_redirect() {
    let (first, second) = servers();
    let rig = Rig::new(&first);
    let (code, text, stderr) = rig.run(&["status"]);
    assert_eq!(code, 0, "{stderr}");
    let line = text
        .lines()
        .find(|l| l.starts_with("backend         default:"))
        .unwrap_or_else(|| panic!("no backend line:\n{text}"));
    assert!(line.contains("unreachable"), "{line}");
    assert!(line.contains("redirect"), "{line}");
    assert!(line.contains("localhost"), "{line}");
    assert!(
        !line.contains(&second.url().replace("127.0.0.1", "localhost")),
        "the line names the URL: {line}"
    );
    let value = rig.json(&["status"]);
    let entry = &value["backends"][0];
    assert_eq!(entry["name"], "default");
    assert_eq!(entry["reachable"], false);
    assert_eq!(entry["detail"], "redirect:localhost");
    assert_eq!(value["backend"]["detail"], "redirect:localhost");
    assert!(
        second.requests().is_empty(),
        "the redirect was followed: {:?}",
        second.requests()
    );
}
