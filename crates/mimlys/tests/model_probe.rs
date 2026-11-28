//! `mimlys model probe` (REQ-MDL-011, THR-40; RFC 015 decision 5 and Amendment 1; handoff 015a
//! item 5), through the binary against the scripted OpenAI-compatible mock server.
//!
//! Every verdict is a state: the class in `probe-results.json` and in the command's output, what the
//! mock received, what the data directory and the user's project hold before and after, and
//! the exit. The mock stands for each class by what it answers: the decisions of a model that acts,
//! asks, gives up, plans, answers in prose, or does not answer at all.

use mimlys_model::mock::{MockConfig, MockServer, Reply, ToolsSupport};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

mod common;
use common::run_bounded;

/// Far longer than anything a test waits for.
const HOLD_MS: u64 = 600_000;

/// What the user's own task and project hold: none of it may reach the model of a probe, and none
/// of it may change.
const USER_GOAL: &str = "SECRET-USER-GOAL-4821";
const USER_FILE: &str = "SECRET-PROJECT-CONTENT-7730";

fn act(tool: &str, arguments: Value) -> Reply {
    Reply::Decision(json!({"decision": "act", "tool": tool, "arguments": arguments}))
}

fn complete() -> Reply {
    Reply::Decision(json!({"decision": "complete", "summary": "done"}))
}

fn read_probe() -> Reply {
    act("read_file", json!({"path": "probe.txt"}))
}

/// The edit as the model most likely makes it: one replacement.
fn replace_red_with_blue() -> Reply {
    act(
        "replace_in_file",
        json!({"path": "probe.txt", "find": "red", "replace": "blue"}),
    )
}

/// A backend of the rig's configuration.
struct Entry {
    name: &'static str,
    url: String,
    locality: &'static str,
    transport: &'static str,
}

impl Entry {
    fn local(name: &'static str, server: &MockServer) -> Self {
        Self {
            name,
            url: server.url().to_owned(),
            locality: "local",
            transport: "envelope",
        }
    }

    fn remote(name: &'static str, server: &MockServer) -> Self {
        Self {
            locality: "remote",
            ..Self::local(name, server)
        }
    }
}

struct Rig {
    dir: tempfile::TempDir,
    config: PathBuf,
    registry: PathBuf,
    /// The registry's holder, when the rig made its own: removed with the rig.
    _own_registry: Option<common::Registry>,
    project: PathBuf,
}

impl Rig {
    fn new(entries: &[Entry], allow_remote: bool) -> Self {
        let own = common::Registry::new();
        let mut rig = Self::in_registry(entries, allow_remote, own.path());
        rig._own_registry = Some(own);
        rig
    }

    /// A rig of its own that shares `registry` with others (the user's runtimes).
    fn in_registry(entries: &[Entry], allow_remote: bool, registry: PathBuf) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("secret.txt"), USER_FILE).unwrap();
        let mut text = format!(
            "config_version = 2\n[runtime]\ndata_dir = {:?}\n",
            dir.path().join("data").to_string_lossy()
        );
        for e in entries {
            text.push_str(&format!(
                "[backends.{name}]\nkind = \"openai_compatible\"\nurl = {url:?}\nmodel = \"test-model\"\nstructured_output = \"none\"\ndecision_transport = \"{transport}\"\nlocality = \"{locality}\"\n",
                name = e.name,
                url = e.url,
                transport = e.transport,
                locality = e.locality,
            ));
        }
        if allow_remote {
            text.push_str("[policy]\nremote_backend_allowed = true\n");
        }
        let config = dir.path().join("config.toml");
        std::fs::write(&config, text).unwrap();
        Self {
            dir,
            config,
            registry,
            _own_registry: None,
            project,
        }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mimlys"));
        cmd.arg("--config")
            .arg(&self.config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_RUN_DIR", &self.registry)
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = run_bounded(&mut self.command(args), self.dir.path());
        (out.code(), out.stdout.clone(), out.stderr.clone())
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }

    /// The user's own task, held in `created` so that no model is asked: its goal and its project
    /// hold what a probe must never send or change.
    fn user_task(&self) -> String {
        let (code, created) = self.json(&[
            "task",
            "new",
            USER_GOAL,
            "--scope",
            self.project.to_str().unwrap(),
            "--hold",
        ]);
        assert_eq!(code, 0, "{created}");
        created["task"]["task_id"].as_str().unwrap().to_owned()
    }

    /// The record `probe-results.json` holds for `backend`.
    fn record(&self, backend: &str) -> Value {
        let text = std::fs::read_to_string(self.data().join("probe-results.json"))
            .unwrap_or_else(|e| panic!("no record: {e}"));
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], 1);
        value["backends"][backend].clone()
    }

    fn has_record_file(&self) -> bool {
        self.data().join("probe-results.json").exists()
    }
}

/// What the data directory and the user's tasks hold, to be compared before and after a probe.
#[derive(Debug, PartialEq, Eq)]
struct State {
    /// The user's tasks as `task list` shows them.
    tasks: Value,
    /// The events of every task, in order (the runtime's own events of a process are not a
    /// task's history).
    events: Vec<Value>,
    /// Every file of the data directory except the store's and the lock's, relative.
    files: BTreeSet<String>,
    /// The project's files and their contents.
    project: Vec<(String, String)>,
    /// The configuration file's bytes.
    config: Vec<u8>,
}

impl Rig {
    fn state(&self) -> State {
        let (_, tasks) = self.json(&["task", "list"]);
        let (_, events) = self.json(&["events"]);
        let events: Vec<Value> = events
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| !e["record"]["task_id"].is_null())
            .cloned()
            .collect();
        let mut files = BTreeSet::new();
        list(&self.data(), &self.data(), &mut files);
        let mut project = Vec::new();
        for entry in std::fs::read_dir(&self.project).unwrap() {
            let path = entry.unwrap().path();
            project.push((
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&path).unwrap(),
            ));
        }
        project.sort();
        State {
            tasks,
            events,
            files,
            project,
            config: std::fs::read(&self.config).unwrap(),
        }
    }

    /// Nothing of a probe is left: no scope under `tmp`, no task but the user's, no workspace of
    /// another, and everything the user had is as it was.
    fn assert_nothing_left(&self, before: &State, what: &str) {
        let after = self.state();
        let scopes: Vec<_> = std::fs::read_dir(self.data().join("tmp"))
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("probe-"))
                    .collect()
            })
            .unwrap_or_default();
        assert!(scopes.is_empty(), "{what}: a scope is left: {scopes:?}");
        assert_eq!(after.tasks, before.tasks, "{what}: the tasks changed");
        assert_eq!(
            after.events, before.events,
            "{what}: a task's history changed"
        );
        assert_eq!(after.project, before.project, "{what}: the project changed");
        assert_eq!(
            after.config, before.config,
            "{what}: the configuration changed"
        );
        assert_eq!(
            after.files, before.files,
            "{what}: files of the data directory changed (beyond the record)"
        );
    }
}

/// Every file under `dir` but the store's and the lock's and the record, relative to `base`.
fn list(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let rel = path
            .strip_prefix(base)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("mimlys.") || rel == "probe-results.json" || rel == "worker.json" {
            continue;
        }
        if path.is_dir() {
            out.insert(format!("{rel}/"));
            list(base, &path, out);
        } else {
            out.insert(rel);
        }
    }
}

fn server(script: Vec<Reply>) -> MockServer {
    MockServer::start(MockConfig::default(), script)
}

/// What every request the mock received says, as one text: to look for what must not be there.
fn everything_sent(server: &MockServer) -> String {
    server
        .requests()
        .iter()
        .map(|r| r.body.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs a probe of the one backend and returns the record of it and the JSON the command printed.
fn probe_one(rig: &Rig, name: Option<&str>) -> (i32, Value, Value) {
    let mut args = vec!["model", "probe"];
    args.extend(name);
    let (code, printed) = rig.json(&args);
    let backend = name.unwrap_or("default");
    let record = if rig.has_record_file() {
        rig.record(backend)
    } else {
        Value::Null
    };
    (code, printed, record)
}

/// The class and detail a record holds.
fn class_of(record: &Value) -> (String, Option<String>) {
    (
        record["class"].as_str().unwrap_or_default().to_owned(),
        record["detail"].as_str().map(str::to_owned),
    )
}

/// One probe against a mock that answers `script`, with the user's task already there; checks the
/// class, that the command printed it, and that nothing of the probe is left whatever the class.
fn expect(script: Vec<Reply>, class: &str, detail: Option<&str>) -> (Rig, MockServer, Value) {
    let server = server(script);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    rig.user_task();
    let before = rig.state();
    let (code, printed, record) = probe_one(&rig, None);
    assert_eq!(code, 0, "{printed}");
    assert_eq!(
        class_of(&record),
        (class.to_owned(), detail.map(str::to_owned)),
        "{record}\n{printed}"
    );
    let shown = &printed["probed"][0];
    assert_eq!(shown["backend"], "default");
    assert_eq!(shown["class"], class);
    assert_eq!(shown["at"], record["at"]);
    assert_eq!(record["model"], "test-model");
    // The text says it too, in words.
    let (_, text, _) = rig.run(&["status"]);
    assert!(
        text.contains(&format!("last probe: {class}")),
        "status shows the last probe: {text}"
    );
    rig.assert_nothing_left(&before, class);
    // Nothing of the user's was sent.
    let sent = everything_sent(&server);
    assert!(
        !sent.contains(USER_GOAL) && !sent.contains(USER_FILE),
        "nothing of the user's is sent: {sent}"
    );
    (rig, server, printed)
}

// ---- the classes ----

/// REQ-MDL-011: a model that makes the edit and says it is done, and whose file the runtime finds as
/// asked, `acts`: the steps, the model time per step, the transport, the model and the date are
/// recorded, the fixed goal is what the model was asked, and nothing is left.
#[test]
fn req_mdl_011_a_model_that_makes_the_edit_acts() {
    let (rig, server, printed) = expect(vec![replace_red_with_blue(), complete()], "acts", None);
    let record = rig.record("default");
    assert_eq!(record["steps"], 2);
    assert!(record["model_ms_per_step"].is_u64(), "{record}");
    assert_eq!(record["transport"], "envelope");
    assert!(record["at"].as_str().unwrap().ends_with('Z'), "{record}");
    // The goal is the fixed one, in the request, and the file is the probe's own.
    let sent = everything_sent(&server);
    assert!(
        sent.contains("Change the word red to blue in the file probe.txt."),
        "{sent}"
    );
    assert_eq!(server.decision_requests().len(), 2);
    assert_eq!(printed["left_out"], json!([]));
    let (_, text, _) = rig.run(&["model", "probe"]);
    // Running it again probes again (the mock has no more replies, so the second class is a
    // different one: what matters here is that the command says what it does).
    assert!(text.contains("probing backend `default`"), "{text}");
}

/// REQ-MDL-011: the edit by `read_file` and `write_file` (the whole content, a line ending or not)
/// acts as well: the verdict is the file, not the tool the model chose.
#[test]
fn req_mdl_011_the_edit_by_writing_the_whole_file_acts_too() {
    expect(
        vec![
            read_probe(),
            act(
                "write_file",
                json!({"path": "probe.txt", "content": "color: blue"}),
            ),
            complete(),
        ],
        "acts",
        None,
    );
}

/// REQ-MDL-011, REQ-MDL-009: under the `tool_calls` transport the model's structured calls drive
/// the same task, and the transport recorded is the one in force.
#[test]
fn req_mdl_011_the_transport_in_force_is_recorded() {
    let server = MockServer::start(
        MockConfig {
            tools: ToolsSupport::Accept,
            ..MockConfig::default()
        },
        vec![
            Reply::call(
                "replace_in_file",
                &json!({"path": "probe.txt", "find": "red", "replace": "blue"}),
            ),
            Reply::call("complete", &json!({"summary": "done"})),
        ],
    );
    let mut entry = Entry::local("default", &server);
    entry.transport = "tool_calls";
    let rig = Rig::new(&[entry], false);
    let (code, _, record) = probe_one(&rig, None);
    assert_eq!(code, 0);
    assert_eq!(class_of(&record).0, "acts", "{record}");
    assert_eq!(record["transport"], "tool_calls");
}

/// REQ-MDL-011: a model that acts and does not get there within the steps is `incomplete`
/// (`not_finished`): it is not filed under a class that says it cannot act.
#[test]
fn req_mdl_011_a_model_that_acts_and_does_not_finish_is_incomplete() {
    let script = (0..12)
        .map(|n| {
            act(
                "replace_in_file",
                json!({"path": "probe.txt", "find": format!("absent-{n}"), "replace": "x"}),
            )
        })
        .collect();
    let (rig, _, _) = expect(script, "incomplete", Some("not_finished"));
    assert!(rig.record("default")["steps"].as_u64().unwrap() <= 8);
}

/// REQ-MDL-011: the verdict is the runtime's, not the model's. A model that writes the wrong
/// content and then says `complete` completes its task, and its file is not as asked: it is
/// `incomplete`, never `acts`, and it says how it ended: `not_verified`. One that says `complete`
/// and never acted is `plans_only`, `not_verified` too.
#[test]
fn req_mdl_011_the_models_word_that_it_is_done_is_not_the_verdict() {
    let (rig, _, _) = expect(
        vec![
            act(
                "write_file",
                json!({"path": "probe.txt", "content": "color: green\n"}),
            ),
            complete(),
        ],
        "incomplete",
        Some("not_verified"),
    );
    assert_eq!(rig.record("default")["steps"], 2);
    expect(vec![complete()], "plans_only", Some("not_verified"));
}

/// REQ-MDL-011 (handoff 015a-5, item 7): a step counts as acting when its decision was an `act`,
/// accepted or rejected after parsing. A model whose every call to `write_file` is invalid (a
/// number for the content, so `schema`) is not `plans_only`: it proposed actions, and badly. The
/// task ends by the guards on its rejections, and the class is `incomplete`, `not_finished`.
/// This runs against the real store: what the verdict reads is what the store kept of the steps.
#[test]
fn req_mdl_011_a_model_whose_every_call_is_invalid_is_incomplete_not_plans_only() {
    let script = (0..12)
        .map(|_| act("write_file", json!({"path": "probe.txt", "content": 5})))
        .collect();
    let (rig, server, _) = expect(script, "incomplete", Some("not_finished"));
    assert!(
        rig.record("default")["steps"].as_u64().unwrap() >= 2,
        "{}",
        rig.record("default")
    );
    assert!(!server.decision_requests().is_empty());
    // Not an `act` that is rejected: a reply the envelope itself refuses is not known to be one.
    let script = (0..12)
        .map(|_| Reply::Decision(json!({"decision": "act", "tool": "write_file"})))
        .collect();
    expect(script, "plans_only", Some("not_finished"));
}

/// REQ-MDL-011: a model that acts and then asks the user is `incomplete` (`asked_user`): nobody
/// answers a probe, and the command does not wait for anyone.
#[test]
fn req_mdl_011_a_model_that_asks_the_user_is_incomplete_asked_user() {
    expect(
        vec![
            read_probe(),
            Reply::Decision(json!({"decision": "ask_user", "question": "Which colour?"})),
        ],
        "incomplete",
        Some("asked_user"),
    );
}

/// REQ-MDL-011: a model that acts and then gives up is `incomplete` (`gave_up`).
#[test]
fn req_mdl_011_a_model_that_gives_up_is_incomplete_gave_up() {
    expect(
        vec![
            read_probe(),
            Reply::Decision(json!({"decision": "give_up", "reason": "cannot"})),
        ],
        "incomplete",
        Some("gave_up"),
    );
}

/// REQ-MDL-011: a model that asks the user, or gives up, before it has proposed any action is
/// `plans_only`, and says how it ended: `asked_user`, `gave_up`.
#[test]
fn req_mdl_011_a_model_that_asks_or_gives_up_first_is_plans_only() {
    expect(
        vec![Reply::Decision(
            json!({"decision": "ask_user", "question": "Which colour?"}),
        )],
        "plans_only",
        Some("asked_user"),
    );
    expect(
        vec![Reply::Decision(
            json!({"decision": "give_up", "reason": "cannot"}),
        )],
        "plans_only",
        Some("gave_up"),
    );
}

/// REQ-MDL-011: a model that plans and never acts, up to the steps it has, is `plans_only`
/// (`not_finished`).
#[test]
fn req_mdl_011_a_model_that_only_plans_is_plans_only() {
    let script = (0..12)
        .map(|n| Reply::Decision(json!({"decision": "plan", "steps": [format!("think {n}")]})))
        .collect();
    let (rig, _, _) = expect(script, "plans_only", Some("not_finished"));
    assert_eq!(rig.record("default")["steps"], 8, "the steps of LIM-E2");
}

/// REQ-MDL-011: replies that are no decision at all, in prose, are `no_structured_call`.
#[test]
fn req_mdl_011_replies_in_prose_are_no_structured_call() {
    let script = (0..6)
        .map(|_| Reply::Content("Sure! I will make the file say blue.".into()))
        .collect();
    expect(script, "no_structured_call", None);
}

/// A URL on which nothing listens.
fn dead_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!(
        "http://127.0.0.1:{}/v1",
        listener.local_addr().unwrap().port()
    )
}

/// REQ-MDL-011 (handoff 015a-5, item 7): a backend that does not answer is `unreachable`, which is
/// said and **not recorded**: the command exits 1, the JSON says so in `not_measured` (and not in
/// `probed`), no record is made, and nothing of the probe is left.
#[test]
fn req_mdl_011_a_backend_that_does_not_answer_is_unreachable_and_nothing_is_recorded() {
    let server = server(vec![]);
    let mut entry = Entry::local("default", &server);
    entry.url = dead_url();
    let rig = Rig::new(&[entry], false);
    rig.user_task();
    let before = rig.state();
    let (code, text, stderr) = rig.run(&["model", "probe"]);
    assert_eq!(code, 1, "{text}\n{stderr}");
    assert!(text.contains("backend `default`: unreachable"), "{text}");
    assert!(text.contains("nothing is recorded"), "{text}");
    let (code, printed) = rig.json(&["model", "probe"]);
    assert_eq!(code, 1, "{printed}");
    assert_eq!(printed["probed"], json!([]), "{printed}");
    assert_eq!(printed["not_measured"][0]["backend"], "default");
    assert_eq!(printed["not_measured"][0]["class"], "unreachable");
    assert!(!rig.has_record_file(), "nothing is recorded for it");
    assert_eq!(server.requests().len(), 0, "the other server saw nothing");
    rig.assert_nothing_left(&before, "unreachable");
}

/// REQ-MDL-011: an engine that was down when it was probed does not erase what the model was
/// measured to do: `acts` stays in the record and in `status`; the probe that found no engine
/// exits 1 and changes nothing of the record, byte for byte.
#[test]
fn req_mdl_011_an_unreachable_probe_leaves_the_last_measurement() {
    let server = server(vec![replace_red_with_blue(), complete()]);
    let up = Entry::local("default", &server);
    let url = up.url.clone();
    let rig = Rig::new(&[up], false);
    let (code, printed, record) = probe_one(&rig, None);
    assert_eq!(code, 0, "{printed}");
    assert_eq!(class_of(&record).0, "acts");
    let kept = std::fs::read(rig.data().join("probe-results.json")).unwrap();

    // The engine is gone: the same configuration, with a URL on which nothing listens.
    let text = std::fs::read_to_string(&rig.config).unwrap();
    std::fs::write(&rig.config, text.replace(&url, &dead_url())).unwrap();
    let (code, printed) = rig.json(&["model", "probe"]);
    assert_eq!(code, 1, "{printed}");
    assert_eq!(printed["not_measured"][0]["class"], "unreachable");
    assert_eq!(
        std::fs::read(rig.data().join("probe-results.json")).unwrap(),
        kept,
        "the record is as it was"
    );
    let (_, text, _) = rig.run(&["status"]);
    let line = text
        .lines()
        .find(|l| l.starts_with("backend         default:"))
        .unwrap();
    assert!(line.contains("unreachable"), "{line}");
    assert!(
        line.contains("last probe: acts, measured on this machine on"),
        "{line}"
    );
}

/// REQ-MDL-011: an `unreachable` that an earlier build of 0.13 recorded is a class this release
/// does not know: `status` shows it as such (not as "not probed", and not dropped), and the next
/// probe of the backend replaces it.
#[test]
fn req_mdl_011_an_unreachable_from_an_earlier_build_is_shown_as_unknown() {
    let server = server(vec![replace_red_with_blue(), complete()]);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    std::fs::create_dir_all(rig.data()).unwrap();
    std::fs::write(
        rig.data().join("probe-results.json"),
        json!({"version": 1, "backends": {"default": {
            "class": "unreachable", "steps": 1, "model": "test-model", "at": "2026-11-22T09:00:00Z"
        }}})
        .to_string(),
    )
    .unwrap();
    let (_, text, _) = rig.run(&["status"]);
    let line = text
        .lines()
        .find(|l| l.starts_with("backend         default:"))
        .unwrap();
    assert!(
        line.contains("last probe: unreachable (unknown to this release), measured on this machine on 2026-11-22"),
        "{line}"
    );
    let (code, printed, record) = probe_one(&rig, None);
    assert_eq!(code, 0, "{printed}");
    assert_eq!(class_of(&record).0, "acts");
}

// ---- the verdict, the scope, the tools ----

/// THR-40, REQ-MDL-011: the probe's task is in the store only while it runs; the user's tasks and
/// their events are exactly as they were, the user's project and the configuration are untouched,
/// and the only change to the data directory is the record. The probe's goal and its one file are
/// all the model is told.
#[test]
fn thr_40_a_probe_changes_nothing_of_the_users() {
    let (rig, server, _) = expect(vec![replace_red_with_blue(), complete()], "acts", None);
    assert!(rig.has_record_file());
    // The request that carried the goal named only the probe's file.
    let sent = everything_sent(&server);
    assert!(sent.contains("probe.txt"), "{sent}");
    assert!(!sent.contains("secret.txt"), "{sent}");
    // The record is owner-only (Unix).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(rig.data().join("probe-results.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}

/// THR-40: the model can name any path it likes; the probe's tools reach the probe's workspace
/// only. A model that tries the user's project, a parent directory and the data directory's
/// own files is refused, and nothing outside the probe's workspace is read or written.
#[test]
fn thr_40_the_probes_tools_reach_nothing_outside_its_workspace() {
    let server = server(vec![
        act("read_file", json!({"path": "../project/secret.txt"})),
        act("read_file", json!({"path": "/etc/hostname"})),
        act(
            "write_file",
            json!({"path": "../../../escaped.txt", "content": "x"}),
        ),
        act("list_dir", json!({"path": "."})),
        act("run_script", json!({"language": "javascript", "code": "1"})),
        complete(),
    ]);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    rig.user_task();
    let before = rig.state();
    let (code, printed, record) = probe_one(&rig, None);
    assert_eq!(code, 0, "{printed}");
    // Every attempt was refused or never offered; the model never read the user's file, and
    // wrote nothing anywhere: the file as it started is not as asked.
    assert_ne!(class_of(&record).0, "acts", "{record}");
    let sent = everything_sent(&server);
    assert!(
        !sent.contains(USER_FILE),
        "the user's file reached the model: {sent}"
    );
    assert!(
        !rig.dir.path().join("escaped.txt").exists() && !rig.data().join("escaped.txt").exists(),
        "nothing was written outside the workspace"
    );
    rig.assert_nothing_left(&before, "a model that tries the project");
}

/// REQ-MDL-011: a probe does not leave its model's request in the user's way: no more than LIM-E2
/// steps are asked of the model, however long the model would go on.
#[test]
fn req_mdl_011_the_model_is_asked_at_most_lim_e2_times() {
    let script = (0..30)
        .map(|n| act("read_file", json!({"path": "probe.txt", "n": n})))
        .collect();
    let server = server(script);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    let (code, _, record) = probe_one(&rig, None);
    assert_eq!(code, 0);
    assert!(
        server.decision_requests().len() <= 8,
        "{}",
        server.decision_requests().len()
    );
    assert!(record["steps"].as_u64().unwrap() <= 8);
}

// ---- which backends ----

/// REQ-MDL-011: without a name, every local backend is probed, one after the other, `default`
/// first; each has its own record, which `status --json` reads.
#[test]
fn req_mdl_011_without_a_name_every_local_backend_is_probed_in_turn() {
    let first = server(vec![replace_red_with_blue(), complete()]);
    let second = server(vec![
        Reply::Decision(json!({"decision": "plan", "steps": ["a"]})),
        Reply::Decision(json!({"decision": "complete", "summary": "done"})),
    ]);
    let rig = Rig::new(
        &[
            Entry::local("small", &second),
            Entry::local("default", &first),
        ],
        false,
    );
    let (code, printed) = rig.json(&["model", "probe"]);
    assert_eq!(code, 0, "{printed}");
    let names: Vec<&str> = printed["probed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["backend"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["default", "small"]);
    assert_eq!(class_of(&rig.record("default")).0, "acts");
    assert_eq!(class_of(&rig.record("small")).0, "plans_only");
    let (_, status) = rig.json(&["status"]);
    let backends = status["backends"].as_array().unwrap();
    let last = |name: &str| {
        backends
            .iter()
            .find(|b| b["name"] == name)
            .map(|b| b["last_probe"]["class"].clone())
            .unwrap()
    };
    assert_eq!(last("default"), "acts");
    assert_eq!(last("small"), "plans_only");
    // Naming one probes that one only, and replaces only its entry.
    first.push(act("read_file", json!({"path": "probe.txt"})));
    let (code, printed) = rig.json(&["model", "probe", "default"]);
    assert_eq!(code, 0, "{printed}");
    assert_eq!(printed["probed"].as_array().unwrap().len(), 1);
    assert_eq!(
        class_of(&rig.record("small")).0,
        "plans_only",
        "small is as it was"
    );
}

/// REQ-MDL-011, THR-40: a remote backend is not probed without a name, and the command says which
/// it left out and how to probe it; named, it is probed after the line that says it sends a fixed
/// mini-task and nothing of the user's, and what is sent is only that task.
#[test]
fn thr_40_a_remote_backend_is_probed_only_when_named() {
    let local = server(vec![replace_red_with_blue(), complete()]);
    let remote = server(vec![replace_red_with_blue(), complete()]);
    let rig = Rig::new(
        &[
            Entry::local("default", &local),
            Entry::remote("cloud", &remote),
        ],
        true,
    );
    rig.user_task();
    let before = rig.state();
    // Without a name.
    let (code, stdout, _) = rig.run(&["model", "probe"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("left out: backend `cloud` is remote")
            && stdout.contains("mimlys model probe cloud"),
        "{stdout}"
    );
    assert_eq!(
        remote.requests().len(),
        0,
        "nothing was sent to the remote backend"
    );
    assert_eq!(local.decision_requests().len(), 2);
    // Named.
    let (code, stdout, stderr) = rig.run(&["model", "probe", "cloud"]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let notice = stdout
        .lines()
        .position(|l| l.contains("fixed mini-task") && l.contains("nothing of yours"))
        .unwrap_or_else(|| panic!("{stdout}"));
    let start = stdout
        .lines()
        .position(|l| l.starts_with("probing backend `cloud`"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(notice < start, "the notice comes first: {stdout}");
    assert_eq!(remote.decision_requests().len(), 2);
    assert_eq!(class_of(&rig.record("cloud")).0, "acts");
    let sent = everything_sent(&remote);
    assert!(
        !sent.contains(USER_GOAL) && !sent.contains(USER_FILE),
        "only the fixed mini-task is sent: {sent}"
    );
    rig.assert_nothing_left(&before, "a remote backend, named");
}

/// REQ-MDL-011, REQ-SEC-010: a remote backend without `policy.remote_backend_allowed` is not
/// configured at all (the file does not load, naming the key), so it cannot be probed, by name or
/// without; and nothing is sent to it.
#[test]
fn thr_40_a_remote_backend_without_the_permission_is_not_probed() {
    let remote = server(vec![replace_red_with_blue(), complete()]);
    let local = server(vec![]);
    let rig = Rig::new(
        &[
            Entry::local("default", &local),
            Entry::remote("cloud", &remote),
        ],
        false,
    );
    for args in [&["model", "probe", "cloud"][..], &["model", "probe"][..]] {
        let (code, stdout, stderr) = rig.run(args);
        assert_eq!(code, 1, "{stdout}");
        assert!(
            stderr.contains("remote_backend_allowed") && stderr.contains("cloud"),
            "{stderr}"
        );
    }
    assert_eq!(remote.requests().len(), 0);
    assert_eq!(local.requests().len(), 0);
    assert!(!rig.has_record_file());
}

/// REQ-MDL-011: an unknown name is refused naming the configured ones, and nothing is made.
#[test]
fn req_mdl_011_an_unknown_backend_is_refused_naming_the_configured_ones() {
    let server = server(vec![]);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    let (code, stdout, stderr) = rig.run(&["model", "probe", "nope"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stderr.contains("`nope`") && stderr.contains("`default`"),
        "{stderr}"
    );
    assert_eq!(server.requests().len(), 0);
}

/// REQ-MDL-011: a record the release cannot read is not replaced; the probe says so before it
/// asks the model anything.
#[test]
fn req_mdl_011_a_record_that_cannot_be_read_stops_the_probe_before_it_begins() {
    let server = server(vec![replace_red_with_blue(), complete()]);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    std::fs::create_dir_all(rig.data()).unwrap();
    std::fs::write(rig.data().join("probe-results.json"), "{\"version\": 9}").unwrap();
    let (code, stdout, stderr) = rig.run(&["model", "probe"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stderr.contains("probe-results.json") && stderr.contains("move the file aside"),
        "{stderr}"
    );
    assert_eq!(server.requests().len(), 0);
    assert_eq!(
        std::fs::read_to_string(rig.data().join("probe-results.json")).unwrap(),
        "{\"version\": 9}"
    );
}

// ---- the writer's lock ----

/// A process that runs a task and holds a request for a long time, so that it holds the store.
struct Holder {
    child: Child,
}

impl Holder {
    fn start(rig: &Rig, server: &MockServer) -> Self {
        let child = rig
            .command(&["run", "hold", "--scope", rig.project.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while server.decision_requests().is_empty() {
            assert!(
                started.elapsed() < Duration::from_secs(120),
                "the holder never asked"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        Self { child }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// LIM-O3, REQ-MDL-011: `model probe` writes, so while another process holds the store it is
/// refused, naming the holder (its pid and since when), in text and in JSON, and it makes nothing:
/// no scope, no record, no request to the model.
#[test]
fn req_mdl_011_a_probe_is_refused_while_another_process_holds_the_store() {
    let server = server(vec![Reply::Slow(HOLD_MS, Box::new(complete()))]);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    let holder = Holder::start(&rig, &server);
    let pid = holder.child.id();
    let asked = server.requests().len();
    let (code, stdout, stderr) = rig.run(&["model", "probe"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stderr.contains(&format!("in use by runtime {pid} since")),
        "the holder is named: {stderr}"
    );
    let (code, printed) = rig.json(&["model", "probe"]);
    assert_eq!(code, 1);
    assert_eq!(printed["error"]["kind"], "conflict", "{printed}");
    assert_eq!(printed["error"]["detail"]["holder_pid"], pid, "{printed}");
    assert!(
        printed["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&pid.to_string()),
        "{printed}"
    );
    assert_eq!(
        server.requests().len(),
        asked,
        "nothing was asked of the model"
    );
    assert!(!rig.has_record_file());
    let scopes = std::fs::read_dir(rig.data().join("tmp"))
        .map(|d| d.filter_map(Result::ok).count())
        .unwrap_or(0);
    assert_eq!(scopes, 0, "no scope was made");
}

// ---- a stop signal (Unix) ----

#[cfg(unix)]
fn interrupt(child: &Child) {
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::INT).expect("the signal");
}

#[cfg(unix)]
fn until(what: &str, mut reached: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !reached() {
        assert!(started.elapsed() < Duration::from_secs(120), "{what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// REQ-MDL-011, REQ-CLI-005: Ctrl-C during the model's request ends the probe within LIM-O1, deletes
/// what it made, records no class, and the terminal says so; the exit is a success (the user
/// asked for it to stop), and nothing of the user's changed.
#[cfg(unix)]
#[test]
fn req_mdl_011_a_stop_signal_ends_the_probe_and_deletes_what_it_made() {
    let server = server(vec![Reply::Slow(
        HOLD_MS,
        Box::new(replace_red_with_blue()),
    )]);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    rig.user_task();
    let before = rig.state();
    let out = rig.dir.path().join("probe.stdout");
    let err = rig.dir.path().join("probe.stderr");
    let mut child = rig
        .command(&["model", "probe"])
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&err).unwrap())
        .spawn()
        .unwrap();
    until("the request never reached the model", || {
        !server.decision_requests().is_empty()
    });
    // The scope and the task exist now.
    assert!(
        std::fs::read_dir(rig.data().join("tmp"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("probe-")),
        "the probe's scope is there while it runs"
    );
    let signalled = std::time::Instant::now();
    interrupt(&child);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            signalled.elapsed()
                < Duration::from_millis(mimlys_core::limits::LIM_O1_STOP_MS as u64 + 3000),
            "the probe did not stop"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(status.success(), "{status:?}");
    let stderr = std::fs::read_to_string(&err).unwrap();
    assert!(
        stderr.contains("stopped:") && stderr.contains("no class was recorded"),
        "{stderr}"
    );
    assert!(!rig.has_record_file(), "no class is recorded");
    rig.assert_nothing_left(&before, "a stopped probe");
}

/// REQ-MDL-011, RFC 013 Amendment 1: a stop request ends the wait for the engine's gate, as it does
/// for a task: while another runtime of the user's holds the one place of the endpoint, the probe
/// waits without sending, Ctrl-C ends the wait within LIM-O1, and nothing was sent, made or recorded.
#[cfg(unix)]
#[test]
fn req_mdl_011_a_stop_signal_ends_the_probes_wait_for_the_engines_gate() {
    let server = server(vec![Reply::Slow(HOLD_MS, Box::new(complete()))]);
    let shared = common::Registry::new();
    let registry = shared.path();
    let holding = Rig::in_registry(&[Entry::local("default", &server)], false, registry.clone());
    let _holder = Holder::start(&holding, &server);
    let probing = Rig::in_registry(&[Entry::local("default", &server)], false, registry.clone());
    let mut child = probing
        .command(&["model", "probe"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    until("the probe never waited for the gate", || {
        let scan = mimlys::registry::Registry::new(
            registry.clone(),
            mimlys::registry::Bounds::default(),
            std::sync::Arc::new(mimlys_core::clock::SystemClock),
        )
        .list()
        .unwrap();
        scan.live.iter().any(|s| {
            s.entry.kind == mimlys::registry::Kind::Gate
                && s.entry.state == mimlys::registry::State::Waiting
                && s.entry.pid == child.id()
        })
    });
    assert_eq!(
        server.decision_requests().len(),
        1,
        "the probe's request is not sent"
    );
    let signalled = std::time::Instant::now();
    interrupt(&child);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            signalled.elapsed()
                < Duration::from_millis(mimlys_core::limits::LIM_O1_STOP_MS as u64 + 3000),
            "the wait for the gate was not ended"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(status.success(), "{status:?}");
    assert_eq!(
        server.decision_requests().len(),
        1,
        "nothing was sent by the probe"
    );
    assert!(!probing.has_record_file());
}

// ---- a crash, at each persistence point ----

/// REQ-MDL-011, REQ-TEST-007: a probe that a crash ended is removed by the next start that writes,
/// whichever persistence point the crash was at, before or after it: the scope, the task with its
/// records and events, its workspace and any unfinished copy. The user's tasks and their events are
/// as they were, no class was recorded, and the store goes on: a probe then runs as it does on a
/// clean store.
#[cfg(feature = "crash-injection")]
#[test]
fn req_mdl_011_a_probe_a_crash_ended_is_removed_by_the_next_start() {
    // The points a probe's task passes (approvals, promotions and blobs it never reaches).
    let points = [
        "create_task",
        "step_started",
        "proposed",
        "validation",
        "authorized",
        "running",
        "execution",
        "action_terminal",
        "step_outcome",
        "task_transition",
        "snapshot",
        "delete",
    ];
    let mut crashed = 0;
    for point in points {
        for phase in ["before", "after"] {
            let target = format!("{point}.{phase}");
            let server = server(vec![replace_red_with_blue(), complete(), complete()]);
            // A second backend for the probe that follows: the first one's script was used by
            // the probe that crashed.
            let after = self::server(vec![replace_red_with_blue(), complete()]);
            let rig = Rig::new(
                &[
                    Entry::local("default", &server),
                    Entry::local("later", &after),
                ],
                false,
            );
            rig.user_task();
            let before = rig.state();
            let mut cmd = rig.command(&["model", "probe", "default"]);
            cmd.env("MIMLYS_CRASH_AT", &target);
            let out = run_bounded(&mut cmd, rig.dir.path());
            if out.status.success() {
                // A point this probe does not pass at that phase: nothing to find.
                println!("not reached: {target}");
                continue;
            }
            crashed += 1;
            assert!(
                !rig.has_record_file(),
                "{target}: a class was recorded by a crash"
            );
            // The next start that writes (a command that opens the store as the writer).
            let (code, stdout, stderr) = rig.run(&["interpreters", "list"]);
            assert_eq!(code, 0, "{target}: {stdout}\n{stderr}");
            assert!(
                !stdout.contains("recover") && !stderr.contains("recover"),
                "{target}: a probe's task is not a recovered task of the user's: {stdout}\n{stderr}"
            );
            rig.assert_nothing_left(&before, &target);
            // And the store works: a probe runs and acts.
            let (code, printed, record) = probe_one(&rig, Some("later"));
            assert_eq!(code, 0, "{target}: {printed}");
            assert_eq!(class_of(&record).0, "acts", "{target}: {record}");
            rig.assert_nothing_left(&before, &format!("{target}, then a probe"));
        }
    }
    assert!(
        crashed >= 2 * points.len() - 4,
        "the crash points of a probe were reached ({crashed} of {})",
        2 * points.len()
    );
}

/// Stops the daemon of a rig when the test ends.
#[cfg(unix)]
struct StopsDaemon<'a>(&'a Rig);

#[cfg(unix)]
impl Drop for StopsDaemon<'_> {
    fn drop(&mut self) {
        let _ = self.0.run(&["daemon", "stop"]);
    }
}

/// REQ-MDL-011, REQ-CLI-005, REQ-PERS-014 (014c-5): **a stop signal reaches a probe that runs in
/// the daemon.** The probe is a request the daemon runs; Ctrl-C in the command is forwarded to it,
/// the daemon ends the probe and deletes what it made, no class is recorded, the terminal says so,
/// the exit is a success, and nothing of the user's changed.
#[cfg(unix)]
#[test]
fn req_mdl_011_a_stop_signal_ends_a_probe_that_runs_in_the_daemon() {
    let server = server(vec![Reply::Slow(
        HOLD_MS,
        Box::new(replace_red_with_blue()),
    )]);
    let rig = Rig::new(&[Entry::local("default", &server)], false);
    let (code, started, said) = rig.run(&["daemon", "start"]);
    assert_eq!(code, 0, "{started}\n{said}");
    let _stops = StopsDaemon(&rig);
    rig.user_task();
    let before = rig.state();
    let out = rig.dir.path().join("probe.stdout");
    let err = rig.dir.path().join("probe.stderr");
    let mut child = rig
        .command(&["model", "probe"])
        .stdout(std::fs::File::create(&out).unwrap())
        .stderr(std::fs::File::create(&err).unwrap())
        .spawn()
        .unwrap();
    until("the request never reached the model", || {
        !server.decision_requests().is_empty()
    });
    // The probe runs in the daemon: its scope and its task are there while it does.
    assert!(
        std::fs::read_dir(rig.data().join("tmp"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with("probe-")),
        "the probe's scope is there while it runs"
    );
    let signalled = std::time::Instant::now();
    interrupt(&child);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            signalled.elapsed()
                < Duration::from_millis(mimlys_core::limits::LIM_O1_STOP_MS as u64 + 3000),
            "the probe did not stop"
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(status.success(), "{status:?}");
    let stderr = std::fs::read_to_string(&err).unwrap();
    assert!(
        stderr.contains("stopped:") && stderr.contains("no class was recorded"),
        "{stderr}"
    );
    assert!(!rig.has_record_file(), "no class is recorded");
    rig.assert_nothing_left(&before, "a stopped probe");
}
