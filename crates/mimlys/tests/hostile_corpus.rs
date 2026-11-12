//! The hostile corpus of REQ-SEC-013, run on every change (REQ-TEST-017): one
//! harness over one directory of case files, `tests/corpus/hostile/<kind>/*.json`
//! (format and how to add a case: `tests/corpus/hostile/README.md`; the
//! guard: `docs/src/maintainers/repository-guards.md` §3a).
//!
//! Each case holds a hostile input and its expected outcome; this file runs
//! it against the production function the `check` field names (the strict
//! parser, the validator, the path canonicalizer, the approval rendering, the
//! context markers, the sanitizer, the runtime's step loop, the built-in
//! tools, the MCP import, the sandbox's compiler) and fails naming the case
//! file. A case added later is a file; this harness is not edited.
//!
//! The cases carry the assertions of the hostile tests that used to sit beside
//! the code (the move table is in the request of handoff 002b, item 2), so this
//! harness is also where these requirements are verified: REQ-SEC-005 and
//! REQ-TEST-005 (traversal, Windows forms, case folding, symbolic links, the
//! existence leak), REQ-SEC-003 (validation and policy errors execute nothing),
//! REQ-SEC-011 (hostile model output and tool results do not end the process),
//! REQ-SEC-002 and INV-02 (model text changes no authority), REQ-DEC-003 and
//! REQ-SEC-009 (malformed input is rejected, never repaired), REQ-DEC-002,
//! REQ-DEC-006, REQ-DEC-007, REQ-WS-011, REQ-CTX-002 and INV-09 (markers are
//! unforgeable), REQ-CTX-008 (invisible characters), REQ-APR-003 (hostile
//! strings in an approval), REQ-TOOL-004 (symbolic links), REQ-SBX-006
//! (malformed modules) and REQ-MCP-003 with THR-25 (hostile descriptions).
//! Two test requirements are verified here in part: REQ-TEST-004 (oversized,
//! duplicated-key, trailing-text and invisible-character decisions get their
//! rejection code at the parser; the same shapes through the loop, executing
//! nothing, are `req_test_004_hostile_decisions_get_their_code_and_execute_nothing`)
//! and REQ-TEST-006 (a rendered approval escapes hostile strings). REQ-PRIV-001
//! (minimization) on the built-in tools' side: a path outside the workspace is
//! denied before any tool runs, and a link out of it is neither read, nor
//! listed with its target, nor searched, so nothing from outside the workspace
//! becomes an observation.

// The symbolic-link scenario runs on Unix only: elsewhere its fields are read by nothing.
#![cfg_attr(not(unix), allow(dead_code))]

use mimlys_core::action::{EffectKind, EffectView};
use mimlys_core::approval::{ApprovalRequest, render};
use mimlys_core::clock::{Clock, FakeClock};
use mimlys_core::context::{SegmentLabel, closing_marker, write_segment};
use mimlys_core::decision::{Decision, parse_strict};
use mimlys_core::hash::hash_bytes;
use mimlys_core::ids::{ActionId, ApprovalId, ScriptedIds, TaskId};
use mimlys_core::observation::sanitize;
use mimlys_core::path::{CaseMode, PlatformRules, WorkspacePath};
use mimlys_core::policy::Policy;
use mimlys_core::rejection::Detail;
use mimlys_core::runtime::{CreateTask, Runtime, RuntimeDeps, Session};
use mimlys_core::spi::fake::{FakePersistence, FakeToolProvider, FakeWorkspaces, ScriptedBackend};
use mimlys_core::spi::{ExecutionOutcome, PersistenceRead};
use mimlys_core::task::ScopeSpec;
use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};
use mimlys_core::validate::permissive::PermissiveHooks;
use mimlys_core::validate::{Outcome, ValidationContext, validate};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod common;

// ---------------------------------------------------------------------------
// The corpus layout: kinds (directories) and the checks each may hold.
// ---------------------------------------------------------------------------

/// Each kind is a directory of `tests/corpus/hostile/`; a case's `check` must be
/// one of its checks. A kind with no case, or a check with no case, fails the
/// harness: a corpus that silently lost a whole family of cases is not run.
const KINDS: &[(&str, &[&str])] = &[
    (
        "decisions",
        &[
            "decision.parse",
            "decision.approval_display",
            "decision.approval_bounded",
            "loop.reject",
            "loop.authority_rejections",
            "loop.authority_approval",
        ],
    ),
    (
        "paths",
        &[
            "path.canonicalize",
            "path.relation",
            "decision.validate",
            "fs.symlinks",
        ],
    ),
    (
        "tool-results",
        &["result.sanitize", "result.segment", "loop.tool_result"],
    ),
    ("mcp-descriptions", &["mcp.import_description"]),
    ("modules", &["sandbox.module"]),
];

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/hostile")
}

// ---------------------------------------------------------------------------
// Inputs: text, bytes, counts.
// ---------------------------------------------------------------------------

/// The named limits a case may use as a count.
fn limit(name: &str) -> Result<usize, String> {
    use mimlys_core::limits::*;
    Ok(match name {
        "LIM_D1_DECISION_BYTES" => LIM_D1_DECISION_BYTES,
        "LIM_I6_APPROVAL_CHARS" => LIM_I6_APPROVAL_CHARS,
        "LIM_I2_TOOL_DESCRIPTION_CHARS" => LIM_I2_TOOL_DESCRIPTION_CHARS,
        "LIM_C1_CONTEXT_CHARS" => LIM_C1_CONTEXT_CHARS,
        "LIM_C2_EXCERPT_CEILING_CHARS" => LIM_C2_EXCERPT_CEILING_CHARS,
        other => return Err(format!("unknown limit `{other}`")),
    })
}

/// A count: a number, or a string `NAME`, `NAME+n` or `NAME*n` over a named limit.
fn count(v: &Value) -> Result<usize, String> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| format!("not a count: {v}")),
        Value::String(s) => {
            if let Some((name, n)) = s.split_once('+') {
                Ok(limit(name)? + n.parse::<usize>().map_err(|e| e.to_string())?)
            } else if let Some((name, n)) = s.split_once('*') {
                Ok(limit(name)? * n.parse::<usize>().map_err(|e| e.to_string())?)
            } else {
                limit(s)
            }
        }
        other => Err(format!("not a count: {other}")),
    }
}

fn hex(s: &str) -> Result<Vec<u8>, String> {
    let digits: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    if digits.chunks(2).any(|pair| pair.len() != 2) {
        return Err(format!("odd number of hex digits in `{s}`"));
    }
    digits
        .chunks(2)
        .map(|pair| {
            u8::from_str_radix(&pair.iter().collect::<String>(), 16).map_err(|e| e.to_string())
        })
        .collect()
}

fn label(name: &str) -> Result<SegmentLabel, String> {
    serde_json::from_value(json!(name)).map_err(|e| format!("unknown segment label `{name}`: {e}"))
}

/// The bytes of an input. A string is its UTF-8 text; an object is serialized
/// compactly; an array is a list of parts: a string, `{"repeat": text, "times":
/// count}`, `{"hex": "ff fe"}`, `{"hex_repeat": "ff", "times": count}` or
/// `{"closing_marker": "observation"}` (the production marker line).
fn bytes(v: &Value) -> Result<Vec<u8>, String> {
    match v {
        Value::String(s) => Ok(s.clone().into_bytes()),
        Value::Object(_) => serde_json::to_vec(v).map_err(|e| e.to_string()),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for part in parts {
                match part {
                    Value::String(s) => out.extend_from_slice(s.as_bytes()),
                    Value::Object(o) => {
                        let keys: BTreeSet<&str> = o.keys().map(String::as_str).collect();
                        let times = || count(o.get("times").ok_or("`times` is missing")?);
                        let text = |k: &str| {
                            o.get(k)
                                .and_then(Value::as_str)
                                .ok_or_else(|| format!("`{k}` is not a string"))
                        };
                        if keys == BTreeSet::from(["repeat", "times"]) {
                            out.extend(text("repeat")?.as_bytes().repeat(times()?));
                        } else if keys == BTreeSet::from(["hex"]) {
                            out.extend(hex(text("hex")?)?);
                        } else if keys == BTreeSet::from(["hex_repeat", "times"]) {
                            out.extend(hex(text("hex_repeat")?)?.repeat(times()?));
                        } else if keys == BTreeSet::from(["closing_marker"]) {
                            out.extend(
                                closing_marker(label(text("closing_marker")?)?).into_bytes(),
                            );
                        } else {
                            return Err(format!("unknown part {part}"));
                        }
                    }
                    other => return Err(format!("unknown part {other}")),
                }
            }
            Ok(out)
        }
        other => Err(format!(
            "an input is a string, an array of parts or an object, not {other}"
        )),
    }
}

fn text(v: &Value) -> Result<String, String> {
    String::from_utf8(bytes(v)?).map_err(|e| format!("input is not UTF-8 text: {e}"))
}

/// Fails with `message` unless `ok`.
fn ensure(ok: bool, message: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(message()) }
}

fn ensure_eq<T: PartialEq + std::fmt::Debug>(
    what: &str,
    actual: T,
    expected: T,
) -> Result<(), String> {
    ensure(actual == expected, || {
        format!("{what}: expected {expected:?}, got {actual:?}")
    })
}

// ---------------------------------------------------------------------------
// Fixtures shared by the checks (the tool definitions of the production tests).
// ---------------------------------------------------------------------------

fn path_tool(name: &str, kind: EffectKind, extra: Vec<Param>) -> ToolDefinition {
    let mut params = vec![Param {
        name: "path".into(),
        ty: ParamType::WorkspacePath,
        optional: false,
        description: "file".into(),
    }];
    params.extend(extra);
    ToolDefinition {
        name: name.into(),
        description: format!("{name}."),
        input: ParamSchema { params },
        output: None,
        effect: EffectView {
            kind,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: 30_000,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

/// `read_file` (with a bounded `max_chars`) and `git_push`, as the validator's tests see them.
fn validator_tools() -> Vec<ToolDefinition> {
    let max_chars = Param {
        name: "max_chars".into(),
        ty: ParamType::Integer { min: 1, max: 1000 },
        optional: true,
        description: "size".into(),
    };
    vec![
        path_tool("read_file", EffectKind::WorkspaceRead, vec![max_chars]),
        ToolDefinition {
            name: "git_push".into(),
            description: "Push.".into(),
            input: ParamSchema::default(),
            output: None,
            effect: EffectView {
                kind: EffectKind::ExternalWrite,
                reversible: false,
                idempotent: false,
            },
            deadline_ms: 60_000,
            profile: ResourceProfile::Host,
            destination_arguments: vec![],
            provider: Provider::Host,
        },
    ]
}

fn delete_tool() -> ToolDefinition {
    let recursive = Param {
        name: "recursive".into(),
        ty: ParamType::Boolean,
        optional: true,
        description: "r".into(),
    };
    let mut t = path_tool("delete_path", EffectKind::WorkspaceWrite, vec![recursive]);
    t.description = "d".into();
    t
}

fn validate_with(tools: &[ToolDefinition], raw: &[u8]) -> Outcome {
    let hooks = PermissiveHooks;
    let ctx = ValidationContext {
        task_running: true,
        pending_user_item: false,
        tools,
        rules: PlatformRules::Posix,
        grants: &hooks,
        effects: &hooks,
        budgets: &hooks,
    };
    validate(&ctx, raw)
}

fn rules(name: &str) -> Result<PlatformRules, String> {
    match name {
        "posix" => Ok(PlatformRules::Posix),
        "windows" => Ok(PlatformRules::Windows),
        other => Err(format!("unknown rule set `{other}`")),
    }
}

// ---------------------------------------------------------------------------
// The runtime fixture (the scripted loop of the runtime's own tests).
// ---------------------------------------------------------------------------

struct Loop {
    runtime: Runtime,
    persistence: Arc<FakePersistence>,
    backend: Arc<ScriptedBackend>,
    tools: Arc<FakeToolProvider>,
    session: Session,
}

fn approval_write_file_policy() -> Policy {
    let mut policy = Policy::shipped();
    policy.profiles.get_mut("workspace").unwrap().approval_tools = vec!["write_file".into()];
    policy
}

fn policy(name: &str) -> Result<Policy, String> {
    match name {
        "shipped" => Ok(Policy::shipped()),
        "approval_write_file" => Ok(approval_write_file_policy()),
        other => Err(format!("unknown policy `{other}`")),
    }
}

impl Loop {
    fn new(backend: Arc<ScriptedBackend>, policy: Policy) -> Self {
        let persistence = Arc::new(FakePersistence::default());
        let tools = Arc::new(FakeToolProvider::new(vec![
            path_tool("read_file", EffectKind::WorkspaceRead, vec![]),
            path_tool("write_file", EffectKind::WorkspaceWrite, vec![]),
            path_tool("list_dir", EffectKind::WorkspaceRead, vec![]),
            path_tool("search_files", EffectKind::WorkspaceRead, vec![]),
            path_tool("replace_in_file", EffectKind::WorkspaceWrite, vec![]),
            path_tool("delete_path", EffectKind::WorkspaceWrite, vec![]),
        ]));
        let workspaces = Arc::new(FakeWorkspaces::new(tools.clone()));
        let runtime = Runtime::new(RuntimeDeps {
            persistence: persistence.clone(),
            backend: backend.clone(),
            workspaces,
            policy,
            ids: Arc::new(ScriptedIds::default()),
            clock: Arc::new(FakeClock::default()),
            runtime_version: "0.1.0-test".into(),
            rules: PlatformRules::Posix,
            probe: mimlys_core::confine::Probe::unavailable("no backend in tests"),
            secrets: vec![],
            context_chars: mimlys_core::limits::LIM_C1_CONTEXT_CHARS,
        })
        .unwrap();
        Self {
            runtime,
            persistence,
            backend,
            tools,
            session: Session {
                id: "s1".into(),
                os_user: "tester".into(),
            },
        }
    }

    fn create(&self, profile: Option<&str>) -> TaskId {
        let request = CreateTask {
            goal: "fix the failing test".into(),
            scope: ScopeSpec {
                root: "/home/u/project".into(),
                include: vec![],
            },
            profile: profile.map(str::to_owned),
            grants: vec![],
            budgets: None,
            deadline_ms: None,
            completion_check: None,
            hold: false,
            from_task: None,
            from_snapshot: None,
        };
        self.runtime
            .create_task(&self.session, request)
            .unwrap()
            .task_id
    }

    /// One step; its outcome as the name the case files use.
    fn step(&self, id: TaskId) -> String {
        let view = block_on(self.runtime.run_step(id)).unwrap();
        serde_json::to_value(view.outcome)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn state(&self, id: TaskId) -> String {
        serde_json::to_value(self.runtime.get_task(id).unwrap().task.state).unwrap()["state"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// What a model could try to change, read back as data.
    fn authority(&self, id: TaskId) -> Value {
        let view = self.runtime.get_task(id).unwrap();
        json!({
            "grants": view.grants,
            "budgets": view.task.budgets,
            "deadline": view.task.deadline,
            "versions": view.task.versions,
            "profile": view.task.profile,
        })
    }
}

/// A minimal executor for the fakes' futures: they never pend.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut cx = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

// ---------------------------------------------------------------------------
// The checks. Each case file's fields are read into one of these structs;
// a field that is not there, or a field that is not known, is an error.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum ParseExpect {
    /// Rejected `malformed` with this problem.
    Malformed(String),
    /// Accepted, of this decision kind.
    Accepted { kind: String },
    /// Accepted as an `act` whose tool name is exactly this (nothing altered it).
    AcceptedAct { tool: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionParse {
    input: Value,
    expect: ParseExpect,
}

fn decision_parse(c: DecisionParse) -> Result<(), String> {
    let raw = bytes(&c.input)?;
    let parsed = parse_strict(&raw);
    match (c.expect, parsed) {
        (ParseExpect::Malformed(problem), Err(r)) => {
            ensure_eq(
                "rejection code",
                serde_json::to_value(r.code).unwrap(),
                json!("malformed"),
            )?;
            match r.detail {
                Detail::Malformed { problem: p, .. } => ensure_eq(
                    "malformed problem",
                    serde_json::to_value(p).unwrap(),
                    json!(problem),
                ),
                other => Err(format!("expected a malformed detail, got {other:?}")),
            }
        }
        (ParseExpect::Malformed(problem), Ok(d)) => Err(format!(
            "expected malformed `{problem}`, but it parsed as `{}`",
            d.kind()
        )),
        (ParseExpect::Accepted { kind }, Ok(d)) => {
            ensure_eq("decision kind", d.kind(), kind.as_str())
        }
        (ParseExpect::AcceptedAct { tool }, Ok(Decision::Act { tool: t, .. })) => {
            ensure_eq("the tool name as parsed", t, tool)
        }
        (ParseExpect::AcceptedAct { .. }, Ok(d)) => Err(format!("not an act: `{}`", d.kind())),
        (_, Err(r)) => Err(format!("expected acceptance, got {r:?}")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidateExpect {
    /// The rejection code of every input.
    code: String,
    /// Whether every input's rejection serializes to the same text.
    identical: bool,
    /// Text the rejection contains.
    contains: Vec<String>,
    /// Text the rejection does not contain.
    lacks: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionValidate {
    inputs: Vec<Value>,
    expect: ValidateExpect,
}

fn decision_validate(c: DecisionValidate) -> Result<(), String> {
    ensure(!c.inputs.is_empty(), || "no inputs".into())?;
    let tools = validator_tools();
    let mut rendered = Vec::new();
    for input in &c.inputs {
        match validate_with(&tools, &bytes(input)?) {
            Outcome::Rejected { rejection, .. } => {
                ensure_eq(
                    "rejection code",
                    serde_json::to_value(rejection.code).unwrap(),
                    json!(c.expect.code),
                )?;
                rendered.push(serde_json::to_string(&rejection).unwrap());
            }
            other => return Err(format!("expected a rejection, got {other:?}")),
        }
    }
    if c.expect.identical {
        ensure(rendered.iter().all(|r| r == &rendered[0]), || {
            format!("the rejections differ: {rendered:?}")
        })?;
    } else {
        ensure(rendered.iter().any(|r| r != &rendered[0]), || {
            "the rejections were expected to differ and are identical".into()
        })?;
    }
    for r in &rendered {
        for needle in &c.expect.contains {
            ensure(r.contains(needle.as_str()), || {
                format!("`{needle}` is missing from {r}")
            })?;
        }
        for needle in &c.expect.lacks {
            ensure(!r.contains(needle.as_str()), || {
                format!("`{needle}` is in {r}")
            })?;
        }
    }
    Ok(())
}

/// A decision authorized against `delete_path`, then rendered for the approver.
fn authorized_delete(raw: &[u8]) -> Result<mimlys_core::validate::AuthorizedAction, String> {
    match validate_with(&[delete_tool()], raw) {
        Outcome::Authorized(a) => Ok(a),
        other => Err(format!("the decision was not authorized: {other:?}")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DisplayExpect {
    lines: usize,
    lacks_chars: Vec<String>,
    contains: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalDisplay {
    input: Value,
    expect: DisplayExpect,
}

fn approval_display(c: ApprovalDisplay) -> Result<(), String> {
    let action = authorized_delete(&bytes(&c.input)?)?;
    let tool = delete_tool();
    let ids = ScriptedIds::default();
    let clock = FakeClock::default();
    let rendered = render(
        &action,
        &tool,
        "/data/workspaces/t1/tree",
        &["tool:delete_path".into()],
        None,
        None,
    );
    let request = ApprovalRequest::new(
        ApprovalId::new(&ids),
        TaskId::new(&ids),
        ActionId::new(&ids),
        hash_bytes(b"action"),
        hash_bytes(b"grants"),
        rendered,
        action.reason(),
        clock.now(),
        None,
    );
    let shown = request.display_text();
    ensure_eq(
        "the number of lines shown",
        shown.lines().count(),
        c.expect.lines,
    )?;
    for ch in &c.expect.lacks_chars {
        ensure(!shown.contains(ch.as_str()), || {
            format!("{ch:?} is shown: {shown}")
        })?;
    }
    for needle in &c.expect.contains {
        ensure(shown.contains(needle.as_str()), || {
            format!("`{needle}` is not shown: {shown}")
        })?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnchangedLine {
    index: usize,
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundedExpect {
    max_chars: Value,
    ellipsis_on_line: usize,
    unchanged_line: UnchangedLine,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalBounded {
    input: Value,
    expect: BoundedExpect,
}

fn approval_bounded(c: ApprovalBounded) -> Result<(), String> {
    let action = authorized_delete(&bytes(&c.input)?)?;
    let rendered = render(&action, &delete_tool(), "/w", &[], None, None);
    let max = count(&c.expect.max_chars)?;
    let chars = rendered.text().chars().count();
    ensure(chars <= max, || {
        format!("{chars} characters shown, the bound is {max}")
    })?;
    let cut = &rendered.lines[c.expect.ellipsis_on_line].value;
    ensure(cut.ends_with('…'), || {
        format!("line {} is not cut: {cut}", c.expect.ellipsis_on_line)
    })?;
    ensure_eq(
        "a fixed line",
        rendered.lines[c.expect.unchanged_line.index].value.as_str(),
        c.expect.unchanged_line.value.as_str(),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopRejectExpect {
    step_outcome: String,
    task_state: String,
    tool_calls: usize,
    /// Persistence lines that must not have been written.
    unwritten: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopReject {
    input: Value,
    expect: LoopRejectExpect,
}

fn loop_reject(c: LoopReject) -> Result<(), String> {
    let backend = Arc::new(ScriptedBackend::default());
    backend.push(Ok(bytes(&c.input)?));
    let h = Loop::new(backend, Policy::shipped());
    let id = h.create(None);
    ensure_eq("step outcome", h.step(id), c.expect.step_outcome)?;
    ensure_eq("task state", h.state(id), c.expect.task_state)?;
    ensure_eq("tool calls", h.tools.calls().len(), c.expect.tool_calls)?;
    let writes = h.persistence.writes();
    for line in &c.expect.unwritten {
        ensure(!writes.contains(&line.as_str()), || {
            format!("a rejected decision wrote `{line}`: {writes:?}")
        })?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityRejectionsExpect {
    step_outcomes: Vec<String>,
    authority_unchanged: bool,
    tool_calls: usize,
    requests: usize,
    model_view_unchanged: bool,
    hidden_from_model: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityRejections {
    decisions: Vec<Value>,
    profile: String,
    expect: AuthorityRejectionsExpect,
}

fn authority_rejections(c: AuthorityRejections) -> Result<(), String> {
    let backend = Arc::new(ScriptedBackend::with_decisions(c.decisions.clone()));
    let h = Loop::new(backend, Policy::shipped());
    let id = h.create(Some(&c.profile));
    let before = h.authority(id);
    let outcomes: Vec<String> = (0..c.decisions.len()).map(|_| h.step(id)).collect();
    ensure_eq("step outcomes", outcomes, c.expect.step_outcomes)?;
    ensure_eq(
        "authority unchanged",
        h.authority(id) == before,
        c.expect.authority_unchanged,
    )?;
    ensure_eq("tool calls", h.tools.calls().len(), c.expect.tool_calls)?;
    let requests = h.backend.requests();
    ensure_eq("backend requests", requests.len(), c.expect.requests)?;
    for (k, r) in requests.iter().enumerate() {
        ensure_eq(
            &format!("request {k}: the system message equals the first request's"),
            r.messages[0] == requests[0].messages[0],
            c.expect.model_view_unchanged,
        )?;
        for hidden in &c.expect.hidden_from_model {
            ensure(!r.messages[0].content.contains(hidden.as_str()), || {
                format!("request {k}: the model was shown `{hidden}`")
            })?;
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityApprovalExpect {
    step_outcome: String,
    pending_approval: String,
    tool_calls: usize,
    authority_unchanged: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityApproval {
    policy: String,
    decisions: Vec<Value>,
    expect: AuthorityApprovalExpect,
}

fn authority_approval(c: AuthorityApproval) -> Result<(), String> {
    let backend = Arc::new(ScriptedBackend::with_decisions(c.decisions));
    let h = Loop::new(backend, policy(&c.policy)?);
    let id = h.create(None);
    let before = h.authority(id);
    ensure_eq("step outcome", h.step(id), c.expect.step_outcome)?;
    let view = h.runtime.get_task(id).unwrap();
    let approval = view
        .pending_approval
        .ok_or("the action was expected to wait for an approval")?;
    ensure_eq(
        "the approval's status",
        serde_json::to_value(approval.status).unwrap(),
        json!(c.expect.pending_approval),
    )?;
    ensure_eq("tool calls", h.tools.calls().len(), c.expect.tool_calls)?;
    ensure_eq(
        "authority unchanged",
        h.authority(id) == before,
        c.expect.authority_unchanged,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum PathExpect {
    /// Refused, with this `path_error` (the serialized name).
    Error(String),
    /// Accepted, as this canonical path.
    Ok(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathCanonicalize {
    input: Value,
    rules: String,
    expect: PathExpect,
}

fn path_canonicalize(c: PathCanonicalize) -> Result<(), String> {
    let result = WorkspacePath::canonicalize_with(&text(&c.input)?, rules(&c.rules)?);
    match (c.expect, result) {
        (PathExpect::Error(name), Err(e)) => ensure_eq(
            "path error",
            serde_json::to_value(&e).unwrap()["path_error"].clone(),
            json!(name),
        ),
        (PathExpect::Error(name), Ok(p)) => Err(format!("expected `{name}`, got the path `{p}`")),
        (PathExpect::Ok(path), Ok(p)) => ensure_eq("canonical path", p.as_str().to_owned(), path),
        (PathExpect::Ok(path), Err(e)) => Err(format!("expected the path `{path}`, got {e:?}")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ByMode {
    sensitive: bool,
    insensitive: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationExpect {
    equal: bool,
    same_as: ByMode,
    /// `a` lies under `b`.
    starts_with: ByMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathRelation {
    a: String,
    b: String,
    rules: String,
    expect: RelationExpect,
}

fn path_relation(c: PathRelation) -> Result<(), String> {
    let rules = rules(&c.rules)?;
    let a = WorkspacePath::canonicalize_with(&c.a, rules).map_err(|e| format!("a: {e:?}"))?;
    let b = WorkspacePath::canonicalize_with(&c.b, rules).map_err(|e| format!("b: {e:?}"))?;
    let (s, i) = (CaseMode::Sensitive, CaseMode::Insensitive);
    ensure_eq("a == b", a == b, c.expect.equal)?;
    ensure_eq(
        "same_as, sensitive",
        a.same_as(&b, s),
        c.expect.same_as.sensitive,
    )?;
    ensure_eq(
        "same_as, insensitive",
        a.same_as(&b, i),
        c.expect.same_as.insensitive,
    )?;
    ensure_eq(
        "starts_with, sensitive",
        a.starts_with(&b, s),
        c.expect.starts_with.sensitive,
    )?;
    ensure_eq(
        "starts_with, insensitive",
        a.starts_with(&b, i),
        c.expect.starts_with.insensitive,
    )
}

// --- symbolic links -------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Link {
    /// Where the link is, in the workspace.
    path: String,
    /// What it points at, relative to the directory outside the workspace ("" is that directory).
    outside: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryExpect {
    path: String,
    r#type: String,
    lacks_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepExpect {
    failed: bool,
    /// JSON pointers into the payload and the values they must equal.
    #[serde(default)]
    payload_equals: BTreeMap<String, Value>,
    /// JSON pointers into the payload and the values they must not equal.
    #[serde(default)]
    payload_differs: BTreeMap<String, Value>,
    /// Notices, as serialized, that must be among the outcome's.
    #[serde(default)]
    notices_include: Vec<Value>,
    /// Entries of a listing that must be there.
    #[serde(default)]
    entries_include: Vec<EntryExpect>,
    /// Path prefixes no entry of a listing may have.
    #[serde(default)]
    entries_lack_prefix: Vec<String>,
    /// Files outside the workspace that must not exist after the step.
    #[serde(default)]
    outside_absent: Vec<String>,
    /// Files outside the workspace that must exist after the step.
    #[serde(default)]
    outside_present: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Step {
    tool: String,
    arguments: Value,
    expect: StepExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Symlinks {
    /// Files of the workspace, by path.
    workspace_files: BTreeMap<String, String>,
    /// Files outside the workspace, by path.
    outside_files: BTreeMap<String, String>,
    links: Vec<Link>,
    steps: Vec<Step>,
}

fn fs_symlinks(name: &str, c: Symlinks) -> Result<(), String> {
    #[cfg(not(unix))]
    {
        let _ = c;
        common::skip(
            name,
            "creates symbolic links with std::os::unix; this platform has none to create",
        );
        Ok(())
    }
    #[cfg(unix)]
    {
        use mimlys_core::spi::{ExecutionContext, ToolProvider};
        use mimlys_exec::builtin::BuiltinTools;
        let _ = name;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let write = |base: &Path, files: &BTreeMap<String, String>| {
            for (rel, content) in files {
                let p = base.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, content).unwrap();
            }
        };
        write(root.path(), &c.workspace_files);
        write(outside.path(), &c.outside_files);
        for l in &c.links {
            let target = if l.outside.is_empty() {
                outside.path().to_path_buf()
            } else {
                outside.path().join(&l.outside)
            };
            std::os::unix::fs::symlink(target, root.path().join(&l.path)).unwrap();
        }
        let tools = BuiltinTools::open(root.path()).map_err(|e| format!("{e:?}"))?;
        let definitions = BuiltinTools::all_definitions();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        for (k, step) in c.steps.iter().enumerate() {
            let at = |m: String| format!("step {k} ({}): {m}", step.tool);
            let hooks = PermissiveHooks;
            let ctx = ValidationContext {
                task_running: true,
                pending_user_item: false,
                tools: &definitions,
                rules: PlatformRules::current(),
                grants: &hooks,
                effects: &hooks,
                budgets: &hooks,
            };
            let raw = serde_json::to_vec(
                &json!({"decision": "act", "tool": step.tool, "arguments": step.arguments}),
            )
            .unwrap();
            let Outcome::Authorized(action) = validate(&ctx, &raw) else {
                return Err(at("not authorized".into()));
            };
            let o: ExecutionOutcome = rt.block_on(tools.execute(
                &action,
                ExecutionContext {
                    deadline_ms: action.deadline_ms(),
                    cancel: mimlys_core::spi::Cancellation::new(),
                },
            ));
            let e = &step.expect;
            ensure_eq(&at("failed".into()), o.failure.is_some(), e.failed)?;
            for (pointer, want) in &e.payload_equals {
                ensure_eq(
                    &at(format!("payload {pointer}")),
                    o.payload.pointer(pointer),
                    Some(want),
                )?;
            }
            for (pointer, not) in &e.payload_differs {
                ensure(o.payload.pointer(pointer) != Some(not), || {
                    at(format!("payload {pointer} is {not}"))
                })?;
            }
            let notices: Vec<Value> = o
                .notices
                .iter()
                .map(|n| serde_json::to_value(n).unwrap())
                .collect();
            for want in &e.notices_include {
                ensure(notices.contains(want), || {
                    at(format!("notice {want} is missing from {notices:?}"))
                })?;
            }
            let entries: Vec<&Value> = o.payload["entries"]
                .as_array()
                .map(|a| a.iter().collect())
                .unwrap_or_default();
            for want in &e.entries_include {
                let entry = entries
                    .iter()
                    .find(|x| x["path"] == want.path)
                    .ok_or_else(|| at(format!("no entry `{}`", want.path)))?;
                ensure_eq(
                    &at(format!("type of `{}`", want.path)),
                    &entry["type"],
                    &json!(want.r#type),
                )?;
                ensure(entry.get(&want.lacks_key).is_none(), || {
                    at(format!("entry `{}` has `{}`", want.path, want.lacks_key))
                })?;
            }
            for prefix in &e.entries_lack_prefix {
                ensure(
                    !entries.iter().any(|x| {
                        x["path"]
                            .as_str()
                            .is_some_and(|p| p.starts_with(prefix.as_str()))
                    }),
                    || at(format!("an entry begins with `{prefix}`")),
                )?;
            }
            for f in &e.outside_absent {
                ensure(!outside.path().join(f).exists(), || {
                    at(format!("outside file `{f}` exists"))
                })?;
            }
            for f in &e.outside_present {
                ensure(outside.path().join(f).exists(), || {
                    at(format!("outside file `{f}` is gone"))
                })?;
            }
        }
        Ok(())
    }
}

// --- tool results ---------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SanitizeExpect {
    clean: String,
    removed: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultSanitize {
    input: Value,
    expect: SanitizeExpect,
}

fn result_sanitize(c: ResultSanitize) -> Result<(), String> {
    let (clean, removed) = sanitize(&text(&c.input)?);
    ensure_eq("cleaned text", clean, c.expect.clean)?;
    ensure_eq("characters removed", removed, c.expect.removed)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentExpect {
    lines: usize,
    opening_starts_with: String,
    body_starts_with: String,
    closing_line: String,
    closing_line_count: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultSegment {
    label: String,
    /// The hostile text, used as an attribute value and as a body field.
    input: Value,
    attribute: String,
    body_field: String,
    expect: SegmentExpect,
}

fn result_segment(c: ResultSegment) -> Result<(), String> {
    let hostile = text(&c.input)?;
    let l = label(&c.label)?;
    let s = write_segment(
        l,
        &[(&c.attribute, &hostile)],
        &json!({ c.body_field.as_str(): hostile }),
    );
    let lines: Vec<&str> = s.lines().collect();
    ensure_eq("lines of the segment", lines.len(), c.expect.lines)?;
    ensure(lines[0].starts_with(&c.expect.opening_starts_with), || {
        format!("opening line: {}", lines[0])
    })?;
    ensure(lines[1].starts_with(&c.expect.body_starts_with), || {
        format!("body line: {}", lines[1])
    })?;
    ensure_eq("closing line", lines[2], c.expect.closing_line.as_str())?;
    ensure_eq(
        "lines equal to the closing marker",
        lines.iter().filter(|x| **x == closing_marker(l)).count(),
        c.expect.closing_line_count,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolResultSpec {
    /// Nesting depth of the payload: arrays around a leaf.
    payload_nesting: usize,
    stdout: Value,
    stderr: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolResultExpect {
    step_outcomes: Vec<String>,
    observations: usize,
    max_stream_chars: Value,
    stdout_lacks_chars: Vec<String>,
    next_request_chars_at_most: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopToolResult {
    /// What the model asks for after the tool result: decisions as JSON objects.
    decisions: Vec<Value>,
    tool_result: ToolResultSpec,
    expect: ToolResultExpect,
}

fn loop_tool_result(c: LoopToolResult) -> Result<(), String> {
    let backend = Arc::new(ScriptedBackend::default());
    let h = Loop::new(backend.clone(), Policy::shipped());
    let mut payload = json!("leaf");
    for _ in 0..c.tool_result.payload_nesting {
        payload = json!([payload]);
    }
    let mut outcome = ExecutionOutcome::succeeded(payload, 1);
    outcome.stdout = bytes(&c.tool_result.stdout)?;
    outcome.stderr = bytes(&c.tool_result.stderr)?;
    h.tools.script("read_file", outcome);
    for d in &c.decisions {
        backend.push(Ok(serde_json::to_vec(d).unwrap()));
    }
    let id = h.create(None);
    let outcomes: Vec<String> = (0..c.decisions.len()).map(|_| h.step(id)).collect();
    ensure_eq("step outcomes", outcomes, c.expect.step_outcomes)?;
    let observations = h.persistence.load_observations(id).unwrap();
    ensure_eq("observations", observations.len(), c.expect.observations)?;
    let max = count(&c.expect.max_stream_chars)?;
    for (name, stream) in [
        ("stdout", &observations[0].stdout),
        ("stderr", &observations[0].stderr),
    ] {
        let n = stream.chars().count();
        ensure(n <= max, || {
            format!("{name} holds {n} characters, the bound is {max}")
        })?;
    }
    for ch in &c.expect.stdout_lacks_chars {
        ensure(!observations[0].stdout.contains(ch.as_str()), || {
            format!("{ch:?} survives in stdout")
        })?;
    }
    let requests = h.backend.requests();
    let sent: usize = requests
        .last()
        .unwrap()
        .messages
        .iter()
        .map(|m| m.content.chars().count())
        .sum();
    let at_most = count(&c.expect.next_request_chars_at_most)?;
    ensure(sent <= at_most, || {
        format!("the next request holds {sent} characters, the bound is {at_most}")
    })
}

// --- MCP descriptions -----------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListedSpec {
    name: String,
    description: Value,
    input_schema: Value,
    annotations: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclarationSpec {
    effect: String,
    idempotent: bool,
    destination_arguments: Vec<String>,
    timeout_ms: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportExpect {
    description_truncated: bool,
    max_description_chars: Value,
    description_lacks_chars: Vec<String>,
    hash_stable: bool,
    hash_differs_from_benign: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpImport {
    server: String,
    tool: ListedSpec,
    declaration: DeclarationSpec,
    /// The description of the same tool when it is not hostile.
    benign_description: String,
    expect: ImportExpect,
}

fn mcp_import_description(c: McpImport) -> Result<(), String> {
    use mimlys_mcp::import::import;
    use mimlys_mcp::server::{Declaration, Listed};
    let declaration = Declaration {
        effect: serde_json::from_value::<EffectKind>(json!(c.declaration.effect))
            .map_err(|e| e.to_string())?,
        idempotent: c.declaration.idempotent,
        destination_arguments: c.declaration.destination_arguments.clone(),
        timeout_ms: c.declaration.timeout_ms,
    };
    let listed = |description: String| Listed {
        name: c.tool.name.clone(),
        description: Some(description),
        input_schema: c.tool.input_schema.clone(),
        output_schema: None,
        annotations: Some(c.tool.annotations.clone()),
    };
    let hostile = listed(text(&c.tool.description)?);
    let imported = import(&c.server, &hostile, Some(&declaration), None);
    ensure_eq(
        "description_truncated",
        imported.record.description_truncated,
        c.expect.description_truncated,
    )?;
    let d = imported.definition.ok_or("the tool is unavailable")?;
    let max = count(&c.expect.max_description_chars)?;
    ensure(d.description.chars().count() <= max, || {
        format!(
            "the description holds {} characters",
            d.description.chars().count()
        )
    })?;
    for ch in &c.expect.description_lacks_chars {
        ensure(!d.description.contains(ch.as_str()), || {
            format!("{ch:?} survives in the description")
        })?;
    }
    let again = import(&c.server, &hostile, Some(&declaration), None);
    ensure_eq(
        "the hash of the same definition is the same",
        again.record.hash == imported.record.hash,
        c.expect.hash_stable,
    )?;
    let benign = import(
        &c.server,
        &listed(c.benign_description.clone()),
        Some(&declaration),
        None,
    );
    ensure_eq(
        "the hash differs from the benign definition's",
        benign.record.hash != imported.record.hash,
        c.expect.hash_differs_from_benign,
    )
}

// --- modules --------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "compile", rename_all = "snake_case")]
enum ModuleExpect {
    /// The compiler refuses the module.
    Rejected,
    /// It compiles, and running it ends like this (the serialized termination).
    Accepted { run: Value },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SandboxModule {
    /// The module: binary as parts, or text format as a string (the sandbox's test-only form).
    module: Value,
    expect: ModuleExpect,
}

fn sandbox_module(c: SandboxModule) -> Result<(), String> {
    use mimlys_exec::sandbox::{Limits, Request, Sandbox};
    let sb = Sandbox::new().map_err(|e| e.to_string())?;
    let compiled = sb.compile(&bytes(&c.module)?);
    match (c.expect, compiled) {
        (ModuleExpect::Rejected, Err(_)) => Ok(()),
        (ModuleExpect::Rejected, Ok(_)) => {
            Err("the module was expected to be refused at compile time".into())
        }
        (ModuleExpect::Accepted { .. }, Err(e)) => Err(format!("the module was refused: {e}")),
        (ModuleExpect::Accepted { run }, Ok(module)) => {
            let root = tempfile::tempdir().unwrap();
            let workspace = root.path().join("tree");
            let tmp = root.path().join("tmp");
            std::fs::create_dir_all(&workspace).unwrap();
            std::fs::create_dir_all(&tmp).unwrap();
            let req = Request {
                args: vec!["prog".into()],
                stdin: Vec::new(),
                workspace,
                read_only: false,
                tmp,
                replay: None,
                limits: Limits::default(),
            };
            let out = sb.run(&module, &req, &sb.interrupt(1000));
            ensure_eq(
                "termination",
                serde_json::to_value(&out.termination).unwrap(),
                run,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Reading the corpus and running it.
// ---------------------------------------------------------------------------

/// Reads `value` as the fields of `T`, naming nothing the check does not know.
fn read<T: for<'de> Deserialize<'de>>(fields: Map<String, Value>) -> Result<T, String> {
    serde_json::from_value(Value::Object(fields)).map_err(|e| e.to_string())
}

/// Runs one case file's content; returns the check it ran.
fn run_case(file_name: &str, content: &str) -> Result<String, String> {
    let mut fields: Map<String, Value> =
        serde_json::from_str(content).map_err(|e| format!("not a JSON object: {e}"))?;
    let check = match fields.remove("check") {
        Some(Value::String(s)) => s,
        _ => return Err("`check` (a string naming the production function) is missing".into()),
    };
    match fields.remove("note") {
        Some(Value::String(s)) if !s.trim().is_empty() => {}
        _ => return Err("`note` (what this case shows, one sentence) is missing".into()),
    }
    if let Some(origin) = fields.remove("origin") {
        ensure(origin.is_string(), || "`origin` is a string".into())?;
    }
    match check.as_str() {
        "decision.parse" => decision_parse(read(fields)?),
        "decision.validate" => decision_validate(read(fields)?),
        "decision.approval_display" => approval_display(read(fields)?),
        "decision.approval_bounded" => approval_bounded(read(fields)?),
        "loop.reject" => loop_reject(read(fields)?),
        "loop.authority_rejections" => authority_rejections(read(fields)?),
        "loop.authority_approval" => authority_approval(read(fields)?),
        "path.canonicalize" => path_canonicalize(read(fields)?),
        "path.relation" => path_relation(read(fields)?),
        "fs.symlinks" => fs_symlinks(file_name, read(fields)?),
        "result.sanitize" => result_sanitize(read(fields)?),
        "result.segment" => result_segment(read(fields)?),
        "loop.tool_result" => loop_tool_result(read(fields)?),
        "mcp.import_description" => mcp_import_description(read(fields)?),
        "sandbox.module" => sandbox_module(read(fields)?),
        other => Err(format!("unknown check `{other}`")),
    }?;
    Ok(check)
}

/// REQ-SEC-013, REQ-TEST-017: every case file of the hostile corpus is run
/// against its production function and its expected outcome is compared; the
/// run fails naming each case file that does not hold, a file that is not a
/// well-formed case, a kind with no case and a check with no case.
#[test]
fn req_sec_013_req_test_017_hostile_corpus() {
    let dir = corpus_dir();
    let mut failures: Vec<String> = Vec::new();
    let mut per_check: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_kind: BTreeMap<&str, usize> = BTreeMap::new();

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("the corpus directory {} cannot be read: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for entry in &entries {
        let name = entry.file_name().unwrap().to_string_lossy().into_owned();
        let known = KINDS.iter().any(|(k, _)| *k == name);
        if !(known && entry.is_dir() || name == "README.md" && entry.is_file()) {
            failures.push(format!(
                "{}: not a kind directory of the corpus (kinds: {:?})",
                entry.display(),
                KINDS.iter().map(|(k, _)| *k).collect::<Vec<_>>()
            ));
        }
    }

    for (kind, checks) in KINDS {
        let kind_dir = dir.join(kind);
        let mut files: Vec<PathBuf> = match std::fs::read_dir(&kind_dir) {
            Ok(rd) => rd.map(|e| e.unwrap().path()).collect(),
            Err(e) => {
                failures.push(format!(
                    "{}: the kind directory cannot be read: {e}",
                    kind_dir.display()
                ));
                continue;
            }
        };
        files.sort();
        for file in files {
            let shown = format!("{kind}/{}", file.file_name().unwrap().to_string_lossy());
            if file.extension().and_then(|e| e.to_str()) != Some("json") || !file.is_file() {
                failures.push(format!(
                    "{shown}: a case is a `.json` file (nothing else lives in a kind directory)"
                ));
                continue;
            }
            *per_kind.entry(kind).or_default() += 1;
            let content = match std::fs::read_to_string(&file) {
                Ok(c) => c,
                Err(e) => {
                    failures.push(format!("{shown}: {e}"));
                    continue;
                }
            };
            // A case counts for its check as soon as it names one, whether or not it holds:
            // a failing case must not be reported as a check that has none.
            let declared = serde_json::from_str::<Value>(&content)
                .ok()
                .and_then(|v| v.get("check").and_then(Value::as_str).map(str::to_owned));
            if let Some(check) = declared {
                if !checks.contains(&check.as_str()) {
                    failures.push(format!("{shown}: the check `{check}` does not belong in `{kind}/` (its checks: {checks:?})"));
                }
                *per_check.entry(check).or_default() += 1;
            }
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_case(&shown, &content)
            }));
            match ran {
                Ok(Ok(_)) => {}
                Ok(Err(message)) => failures.push(format!("{shown}: {message}")),
                Err(panic) => {
                    let message = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                        .unwrap_or_default();
                    failures.push(format!("{shown}: the production code panicked: {message}"));
                }
            }
        }
    }

    for (kind, checks) in KINDS {
        if per_kind.get(kind).copied().unwrap_or(0) == 0 {
            failures.push(format!("{kind}/: the corpus has no case of this kind"));
        }
        for check in *checks {
            if per_check.get(*check).copied().unwrap_or(0) == 0 {
                failures.push(format!(
                    "{kind}/: the corpus has no case of the check `{check}`"
                ));
            }
        }
    }

    let total: usize = per_kind.values().sum();
    eprintln!("hostile corpus: {total} cases, {per_kind:?}");
    assert!(
        failures.is_empty(),
        "{} of {total} hostile corpus checks failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
