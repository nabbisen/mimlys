//! The adapter SPI (external design §3; REQ-ARCH-004, REQ-MDL-001): the
//! model-backend interface (`ModelBackend`), the tool-provider interface
//! (`ToolProvider`, through which built-in, sandbox, host and MCP tools are
//! executed), the workspace interface (`WorkspaceProvider`, which widened the
//! baseline's `SnapshotStore` in RFC 006) and the persistence interface
//! (`Persistence`). There is no `Executor` or `ApprovalChannel` trait here:
//! execution is `ToolProvider::execute`, and the approval prompt is the host
//! crate's `Interaction` (`mimlys::prompt`).

use crate::action::{Action, Execution, ExecutionMetrics, FailureCause, Termination};
use crate::approval::ApprovalRequest;
use crate::event::EventRecord;
use crate::grant::Grant;
use crate::ids::{ActionId, BlobId, PromotionId, SnapshotId, TaskId};
pub use crate::native::DecisionTransport;
use crate::observation::Notice;
use crate::observation::Observation;
use crate::path::CaseMode;
use crate::question::Question;
use crate::snapshot::{
    Manifest, Promotion, PromotionPlan, RestoreReport, SnapshotReason, SnapshotRecord,
    WorkspaceState,
};
use crate::step::Step;
use crate::task::{PlanVersion, ScopeSpec, Task};
use crate::tool::ToolDefinition;
use crate::validate::AuthorizedAction;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A boxed, sendable future, so that providers can be used through `dyn`.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A cancellation flag shared between the loop and an execution.
#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    /// A fresh, uncancelled flag.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The role of a message in a model request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// System instructions.
    System,
    /// User-side content (goal, observations, feedback).
    User,
    /// The model's earlier output.
    Assistant,
    /// The answer to an earlier call of the model (the `tool_calls` transport).
    Tool,
}

/// A structured call the model made, carried by an `assistant` message in
/// the `tool_calls` transport (external design §15.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The call's id; the `tool` message that answers it names the same id.
    pub id: String,
    /// The function's name.
    pub name: String,
    /// The arguments as one JSON object, in text.
    pub arguments: String,
}

/// One message of a model request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Role.
    pub role: Role,
    /// Content.
    pub content: String,
    /// The call an `assistant` message carries (`tool_calls` transport only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<ToolCall>,
    /// The id of the call a `tool` message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

impl Message {
    /// A message of plain text, as the envelope transport sends every message.
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            call: None,
            call_id: None,
        }
    }

    /// An `assistant` message carrying one call and no text.
    pub fn call(call: ToolCall) -> Self {
        Self {
            role: Role::Assistant,
            content: String::new(),
            call: Some(call),
            call_id: None,
        }
    }

    /// The `tool` message answering the call `id`.
    pub fn answer(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            call: None,
            call_id: Some(id.into()),
        }
    }
}

/// A request for one decision (REQ-MDL-001).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerateRequest {
    /// The assembled context.
    pub messages: Vec<Message>,
    /// The decision schema version the model must follow.
    pub schema_version: u32,
    /// Maximum bytes the backend may return (LIM-D1); more is cut and flagged.
    pub max_bytes: usize,
    /// Deadline in milliseconds.
    pub deadline_ms: i64,
    /// How the decision is asked for and read (REQ-MDL-009).
    #[serde(default)]
    pub transport: DecisionTransport,
    /// The functions offered, in the `tool_calls` transport; empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<crate::native::Function>,
}

/// Why a backend could not answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum BackendError {
    /// The backend could not be reached or answered with an error.
    #[error("backend unavailable: {detail}")]
    Unavailable {
        /// A fixed-vocabulary detail.
        detail: String,
    },
    /// The deadline passed.
    #[error("backend timeout")]
    Timeout,
    /// Cancelled.
    #[error("backend request cancelled")]
    Cancelled,
    /// A scripted backend ran out of responses.
    #[error("script exhausted")]
    ScriptExhausted,
}

/// Usage reported by a backend when available (REQ-MDL-006).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens.
    pub prompt_tokens: Option<u64>,
    /// Completion tokens.
    pub completion_tokens: Option<u64>,
    /// Prompt tokens served from a cache.
    pub cached_tokens: Option<u64>,
}

/// Whether a backend runs on this host (REQ-SEC-010).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locality {
    /// On this host.
    Local,
    /// Elsewhere; needs the administrator's explicit permission.
    Remote,
}

/// The model backend (REQ-MDL-001). Output is bytes until validated.
pub trait ModelBackend: Send + Sync {
    /// What this backend is: kind, model and locality (REQ-SEC-010).
    fn describe(&self) -> BackendDescriptor;

    /// Produces the model's raw output for one decision.
    fn generate<'a>(
        &'a self,
        request: GenerateRequest,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>>;

    /// The backend's locality.
    fn locality(&self) -> Locality {
        self.describe().locality
    }

    /// Usage of the last request, when reported.
    fn last_usage(&self) -> Option<Usage> {
        None
    }

    /// The decision transport in force (REQ-MDL-009): the runtime assembles
    /// the context for it before every decision. A backend that offers one
    /// transport only keeps the default. `Err` means the transport could not
    /// be settled now (a configured `tool_calls` the engine refuses, an
    /// engine not answering); the step then halts `backend_unavailable`.
    fn decision_transport<'a>(
        &'a self,
        cancel: Cancellation,
    ) -> BoxFuture<'a, Result<DecisionTransport, BackendError>> {
        let _ = cancel;
        Box::pin(async { Ok(DecisionTransport::Envelope) })
    }

    /// The characters of text the last reply carried beside its one structured
    /// call, when it had both: not part of the decision, recorded in
    /// `decision.received` and nowhere else (REQ-DEC-003).
    fn last_text_chars(&self) -> Option<u64> {
        None
    }
}

/// A backend's identity, recorded at start and on every task's versions
/// (REQ-SEC-010): the kind (`fake`, `openai_compatible`), the model name
/// when the kind has one, and the locality.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendDescriptor {
    /// The kind.
    pub kind: String,
    /// The model name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Where it runs.
    pub locality: Locality,
}

/// What a tool provider receives beside the action.
#[derive(Clone, Debug)]
pub struct ExecutionContext {
    /// Effective deadline in milliseconds (REQ-TOOL-006).
    pub deadline_ms: i64,
    /// Cancellation flag (REQ-TOOL-007).
    pub cancel: Cancellation,
}

/// What a tool provider returns (external design §7.4, §8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionOutcome {
    /// How the execution ended.
    pub termination: Termination,
    /// The failure the provider itself classified, if any; otherwise the
    /// loop derives it from the termination.
    pub failure: Option<FailureCause>,
    /// The structured result.
    pub payload: Value,
    /// Raw standard output.
    pub stdout: Vec<u8>,
    /// Raw standard error.
    pub stderr: Vec<u8>,
    /// Notices the provider attaches (truncations, omitted symlinks).
    pub notices: Vec<Notice>,
    /// Measurements.
    pub metrics: ExecutionMetrics,
}

impl ExecutionOutcome {
    /// A successful outcome with a payload and no streams.
    pub fn succeeded(payload: Value, wall_ms: i64) -> Self {
        Self {
            termination: Termination::Exited { exit_code: 0 },
            failure: None,
            payload,
            stdout: Vec::new(),
            stderr: Vec::new(),
            notices: Vec::new(),
            metrics: ExecutionMetrics {
                wall_ms,
                ..ExecutionMetrics::default()
            },
        }
    }

    /// A tool-level failure with a fixed-vocabulary detail.
    pub fn tool_error(detail: &str, payload: Value, wall_ms: i64) -> Self {
        Self {
            termination: Termination::Exited { exit_code: 1 },
            failure: Some(FailureCause::ToolError {
                detail: detail.to_owned(),
            }),
            payload,
            stdout: Vec::new(),
            stderr: Vec::new(),
            notices: Vec::new(),
            metrics: ExecutionMetrics {
                wall_ms,
                ..ExecutionMetrics::default()
            },
        }
    }

    /// An outcome that ended by deadline.
    pub fn timed_out(wall_ms: i64) -> Self {
        Self {
            termination: Termination::Deadline,
            failure: Some(FailureCause::Timeout),
            payload: Value::Null,
            stdout: Vec::new(),
            stderr: Vec::new(),
            notices: Vec::new(),
            metrics: ExecutionMetrics {
                wall_ms,
                ..ExecutionMetrics::default()
            },
        }
    }

    /// An outcome in which the infrastructure failed (`host_fault`).
    pub fn host_fault(detail: crate::action::HostFaultDetail, wall_ms: i64) -> Self {
        Self {
            termination: Termination::HostFault { detail },
            failure: None,
            payload: Value::Null,
            stdout: Vec::new(),
            stderr: Vec::new(),
            notices: Vec::new(),
            metrics: ExecutionMetrics {
                wall_ms,
                ..ExecutionMetrics::default()
            },
        }
    }

    /// An outcome that was cancelled.
    pub fn cancelled(wall_ms: i64) -> Self {
        Self {
            termination: Termination::Cancelled,
            failure: None,
            payload: Value::Null,
            stdout: Vec::new(),
            stderr: Vec::new(),
            notices: Vec::new(),
            metrics: ExecutionMetrics {
                wall_ms,
                ..ExecutionMetrics::default()
            },
        }
    }
}

/// A source of tools (REQ-ARCH-004). `execute` accepts only an
/// `AuthorizedAction`, which only the validator can construct (REQ-ARCH-005).
pub trait ToolProvider: Send + Sync {
    /// The tools this provider offers.
    fn definitions(&self) -> Vec<ToolDefinition>;

    /// The import records of the external tools among them (RFC 009 §2).
    fn imports(&self) -> std::collections::BTreeMap<String, crate::tool::ImportRecord> {
        Default::default()
    }

    /// Checks the pins at a step boundary (RFC 009 §7): refreshes a dirty
    /// server list and returns the tools whose definition no longer
    /// matches `pinned`, which the provider then omits from `definitions`.
    fn refresh_pins(
        &self,
        pinned: &std::collections::BTreeMap<String, crate::tool::ImportRecord>,
    ) -> Vec<String> {
        let _ = pinned;
        Vec::new()
    }

    /// Events the provider produced since the last call (server starts and stops).
    fn drain_events(&self) -> Vec<crate::event::Event> {
        Vec::new()
    }

    /// Runs an authorized action.
    fn execute<'a>(
        &'a self,
        action: &'a AuthorizedAction,
        context: ExecutionContext,
    ) -> BoxFuture<'a, ExecutionOutcome>;
}

/// A workspace could not be provided.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum WorkspaceError {
    /// The scope root does not exist or is not a directory.
    #[error("scope root is not a directory")]
    ScopeNotFound,
    /// The scope is larger than LIM-W1 or has more files than LIM-W2.
    #[error("scope too large: {bytes} bytes, {files} files")]
    TooLarge {
        /// Measured bytes.
        bytes: u64,
        /// Measured files.
        files: u64,
    },
    /// The scope lies inside the runtime's data directory.
    #[error("scope inside the data directory")]
    InsideDataDir,
    /// The runtime's data directory lies inside the scope.
    #[error("scope contains the data directory")]
    ContainsDataDir,
    /// No workspace exists for the task.
    #[error("no workspace for the task")]
    Missing,
    /// Storage failed.
    #[error("workspace storage failure: {detail}")]
    Storage {
        /// A fixed-vocabulary detail.
        detail: String,
    },
    /// No such snapshot or promotion for the task.
    #[error("no such {what} for the task")]
    NotFound {
        /// What was looked for.
        what: String,
    },
    /// A promotion target differs from the base (REQ-WS-007).
    #[error("promotion refused: {} file(s) changed in the project since the task started", paths.len())]
    Conflict {
        /// The conflicting paths.
        paths: Vec<String>,
    },
    /// The operation is not valid now (the plan is stale, the task not terminal).
    #[error("refused: {reason}")]
    Refused {
        /// Why.
        reason: String,
    },
}

/// What a materialized workspace is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    /// Absolute path of the working tree.
    pub root: String,
    /// Case sensitivity of its filesystem.
    pub case_mode: CaseMode,
    /// Bytes materialized.
    pub bytes: u64,
    /// Files materialized.
    pub files: u64,
    /// Symbolic links omitted because their target leaves the scope (`symlink_omitted`).
    #[serde(default)]
    pub symlinks_omitted: u64,
}

/// Task workspaces: materialization, tools over the tree, quota (external design §14; REQ-WS-002, REQ-WS-008).
pub trait WorkspaceProvider: Send + Sync {
    /// Materializes the scope into the task's workspace.
    fn materialize(
        &self,
        task_id: TaskId,
        scope: &ScopeSpec,
    ) -> Result<WorkspaceInfo, WorkspaceError>;
    /// An existing workspace.
    fn open(&self, task_id: TaskId) -> Result<WorkspaceInfo, WorkspaceError>;
    /// The built-in tools bound to the task's workspace.
    fn tools(&self, task_id: TaskId) -> Result<std::sync::Arc<dyn ToolProvider>, WorkspaceError>;
    /// Bytes the workspace may still grow by, when tracked.
    fn remaining_bytes(&self, task_id: TaskId) -> Result<Option<u64>, WorkspaceError>;
    /// Prepares a snapshot of the tree: every file hashed, objects and the
    /// manifest written durably, the journal open (REQ-WS-004, REQ-PERS-009).
    /// When the manifest equals `previous`'s, that record is reused and no
    /// files are written. The runtime records the row, then calls `seal`.
    fn snapshot(
        &self,
        task_id: TaskId,
        snapshot_id: SnapshotId,
        reason: SnapshotReason,
        action_id: Option<ActionId>,
        previous: Option<&SnapshotRecord>,
        now: crate::clock::Instant,
    ) -> Result<PreparedSnapshot, WorkspaceError>;
    /// Closes the journal of a recorded snapshot.
    fn seal(&self, snapshot_id: SnapshotId) -> Result<(), WorkspaceError>;
    /// Makes the tree equal to the snapshot's manifest (REQ-WS-006).
    fn restore(
        &self,
        task_id: TaskId,
        snapshot: &SnapshotRecord,
    ) -> Result<RestoreReport, WorkspaceError>;
    /// A snapshot's manifest, verified against the recorded hash.
    fn manifest(&self, snapshot: &SnapshotRecord) -> Result<Manifest, WorkspaceError>;
    /// Materializes a new task's tree from a snapshot and prepares its `fork` snapshot (REQ-TASK-004).
    fn materialize_from(
        &self,
        task_id: TaskId,
        snapshot_id: SnapshotId,
        from: &SnapshotRecord,
        scope: &ScopeSpec,
        now: crate::clock::Instant,
    ) -> Result<(WorkspaceInfo, PreparedSnapshot), WorkspaceError>;
    /// Phase 1 of a promotion: the diff against the base, conflicts refused (REQ-WS-007).
    fn promotion_plan(
        &self,
        task_id: TaskId,
        base: &SnapshotRecord,
        project_root: &str,
    ) -> Result<PromotionPlan, WorkspaceError>;
    /// Phase 2: backs the targets up (a prepared `backup` snapshot) and applies the plan.
    fn promote(
        &self,
        task_id: TaskId,
        plan: &PromotionPlan,
        project_root: &str,
        backup_id: SnapshotId,
        promotion_id: PromotionId,
        now: crate::clock::Instant,
    ) -> Result<(PreparedSnapshot, Promotion), WorkspaceError>;
    /// Reverses a promotion from its backup.
    fn revert_promotion(
        &self,
        promotion: &Promotion,
        backup: &SnapshotRecord,
    ) -> Result<RestoreReport, WorkspaceError>;
    /// Bytes of the working tree.
    fn tree_bytes(&self, task_id: TaskId) -> Result<u64, WorkspaceError>;
    /// Removes the files the database no longer references (RFC 006 §6).
    fn delete_files(&self, task_id: TaskId, rows: &DeletedRows) -> Result<(), WorkspaceError>;
}

/// A snapshot whose files are durable and whose journal is open until the
/// runtime has recorded the row and called `seal`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedSnapshot {
    /// The record to store.
    pub record: SnapshotRecord,
    /// The objects the manifest references, with sizes (reference counts).
    pub objects: Vec<(crate::hash::Hash, u64)>,
    /// Whether an existing record was reused (nothing to record or seal).
    pub reused: bool,
}

/// What a deletion removed from the database (RFC 006 §6).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeletedRows {
    /// Objects whose reference count reached zero; their files may go.
    pub objects_at_zero: Vec<crate::hash::Hash>,
    /// Snapshot ids whose manifests may go.
    pub snapshots: Vec<SnapshotId>,
    /// Blob ids whose files may go.
    pub blobs: Vec<BlobId>,
}

/// Counts derived from events and records (REQ-OBS-003), with the context
/// and model measures of REQ-CTX-010 summed over steps.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// Tasks by state name.
    pub tasks_by_state: std::collections::BTreeMap<String, u64>,
    /// Events by name.
    pub events_by_name: std::collections::BTreeMap<String, u64>,
    /// Rejections by code.
    pub rejections_by_code: std::collections::BTreeMap<String, u64>,
    /// Halts by reason.
    pub halts_by_reason: std::collections::BTreeMap<String, u64>,
    /// Approvals by decision.
    pub approvals_by_decision: std::collections::BTreeMap<String, u64>,
    /// Finished actions by status.
    pub actions_by_status: std::collections::BTreeMap<String, u64>,
    /// The context and model measures over measured steps.
    #[serde(default)]
    pub context: ContextMetrics,
}

/// Averages and ratios over the steps that carry measures (REQ-CTX-010).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ContextMetrics {
    /// Steps with measures.
    pub measured_steps: u64,
    /// Average characters per request.
    pub context_chars_avg: Option<u64>,
    /// Σ prefix / Σ context over steps after a task's first: what a prefix cache may reuse.
    pub prefix_ratio: Option<f64>,
    /// Σ characters before the rules / Σ after.
    pub compression_ratio: Option<f64>,
    /// Steps whose retained set alone exceeded the budget.
    pub over_budget_steps: u64,
    /// Steps summarized, summed.
    pub compressed_steps: u64,
    /// Summaries dropped, summed.
    pub dropped_steps: u64,
    /// Average model wall time, in milliseconds.
    pub model_ms_avg: Option<i64>,
    /// Prompt tokens reported by the backend, summed.
    pub prompt_tokens: u64,
    /// Completion tokens reported, summed.
    pub completion_tokens: u64,
    /// Cached prompt tokens reported, summed.
    pub cached_tokens: u64,
    /// Σ context characters / Σ prompt tokens over steps that reported usage:
    /// the number to set `context_chars` from a model's window.
    pub chars_per_prompt_token: Option<f64>,
}

/// The running sums behind [`ContextMetrics`].
#[derive(Clone, Debug, Default)]
pub struct ContextSums {
    steps: u64,
    chars: u64,
    chars_before: u64,
    prefix: u64,
    context_after_first: u64,
    over_budget: u64,
    compressed: u64,
    dropped: u64,
    model_ms: i64,
    model_steps: u64,
    prompt: u64,
    completion: u64,
    cached: u64,
    chars_with_usage: u64,
}

impl ContextSums {
    /// Adds one step's measures, when it has any.
    pub fn add(&mut self, step: &crate::step::Step) {
        let Some(m) = &step.measures else { return };
        self.steps += 1;
        self.chars += m.context.context_chars;
        self.chars_before += m.context.chars_before_fit;
        if step.index > 0 {
            self.prefix += m.context.prefix_chars;
            self.context_after_first += m.context.context_chars;
        }
        self.over_budget += u64::from(m.context.over_budget);
        self.compressed += u64::from(m.context.compressed_steps);
        self.dropped += u64::from(m.context.dropped_steps);
        if let Some(ms) = m.model_ms {
            self.model_ms += ms;
            self.model_steps += 1;
        }
        if let Some(u) = &m.usage {
            self.prompt += u.prompt_tokens.unwrap_or(0);
            self.completion += u.completion_tokens.unwrap_or(0);
            self.cached += u.cached_tokens.unwrap_or(0);
            if u.prompt_tokens.is_some_and(|p| p > 0) {
                self.chars_with_usage += m.context.context_chars;
            }
        }
    }

    /// The derived metrics.
    pub fn finish(&self) -> ContextMetrics {
        let ratio = |num: u64, den: u64| (den > 0).then(|| num as f64 / den as f64);
        ContextMetrics {
            measured_steps: self.steps,
            context_chars_avg: (self.steps > 0).then(|| self.chars / self.steps),
            prefix_ratio: ratio(self.prefix, self.context_after_first),
            compression_ratio: ratio(self.chars_before, self.chars),
            over_budget_steps: self.over_budget,
            compressed_steps: self.compressed,
            dropped_steps: self.dropped,
            model_ms_avg: (self.model_steps > 0).then(|| self.model_ms / self.model_steps as i64),
            prompt_tokens: self.prompt,
            completion_tokens: self.completion,
            cached_tokens: self.cached,
            chars_per_prompt_token: ratio(self.chars_with_usage, self.prompt),
        }
    }
}

/// A scripted tool provider for tests (REQ-TEST-002).
#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Where a fake adapter panics (REQ-SEC-011, REQ-ENG-004).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PanicAt {
        /// In the call itself, before a future is returned.
        Call,
        /// When the returned future is first polled.
        Poll,
        /// Never: the future stays pending, and panics when it is dropped
        /// (a call given up while it is still running).
        Drop,
    }

    /// A future that never completes and panics when dropped.
    struct PanicsWhenDropped(String);

    impl std::future::Future for PanicsWhenDropped {
        type Output = std::convert::Infallible;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Pending
        }
    }

    impl Drop for PanicsWhenDropped {
        fn drop(&mut self) {
            panic!("{}", self.0);
        }
    }

    /// Methods of a fake adapter that panic, by name, with their messages.
    type PanickingMethods = Mutex<std::collections::HashMap<&'static str, String>>;

    fn panic_if_set(methods: &PanickingMethods, method: &'static str) {
        let message = methods.lock().expect("fake lock").get(method).cloned();
        if let Some(message) = message {
            panic!("{message}");
        }
    }

    /// Returns scripted outcomes per tool and records every call.
    #[derive(Debug, Default)]
    pub struct FakeToolProvider {
        panic: Mutex<Option<(PanicAt, String)>>,
        panicking_methods: PanickingMethods,
        definitions: Vec<ToolDefinition>,
        outcomes: Mutex<std::collections::HashMap<String, VecDeque<ExecutionOutcome>>>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl FakeToolProvider {
        /// A provider offering `definitions`.
        pub fn new(definitions: Vec<ToolDefinition>) -> Self {
            Self {
                definitions,
                ..Self::default()
            }
        }

        /// From now on, `execute` panics with `message` at `at`.
        pub fn panic_on_execute(&self, at: PanicAt, message: &str) {
            *self.panic.lock().expect("fake lock") = Some((at, message.to_owned()));
        }

        /// From now on, the named method (`definitions`, `imports`,
        /// `refresh_pins` or `drain_events`) panics with `message`.
        pub fn panic_in(&self, method: &'static str, message: &str) {
            self.panicking_methods
                .lock()
                .expect("fake lock")
                .insert(method, message.to_owned());
        }

        /// No method panics any more.
        pub fn stop_panicking(&self) {
            *self.panic.lock().expect("fake lock") = None;
            self.panicking_methods.lock().expect("fake lock").clear();
        }

        /// Queues an outcome for `tool`.
        pub fn script(&self, tool: &str, outcome: ExecutionOutcome) {
            self.outcomes
                .lock()
                .expect("fake lock")
                .entry(tool.to_owned())
                .or_default()
                .push_back(outcome);
        }

        /// The calls made so far: tool name and canonical arguments.
        pub fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().expect("fake lock").clone()
        }
    }

    impl ToolProvider for FakeToolProvider {
        fn definitions(&self) -> Vec<ToolDefinition> {
            panic_if_set(&self.panicking_methods, "definitions");
            self.definitions.clone()
        }

        fn imports(&self) -> std::collections::BTreeMap<String, crate::tool::ImportRecord> {
            panic_if_set(&self.panicking_methods, "imports");
            Default::default()
        }

        fn refresh_pins(
            &self,
            _pinned: &std::collections::BTreeMap<String, crate::tool::ImportRecord>,
        ) -> Vec<String> {
            panic_if_set(&self.panicking_methods, "refresh_pins");
            Vec::new()
        }

        fn drain_events(&self) -> Vec<crate::event::Event> {
            panic_if_set(&self.panicking_methods, "drain_events");
            Vec::new()
        }

        fn execute<'a>(
            &'a self,
            action: &'a AuthorizedAction,
            _context: ExecutionContext,
        ) -> BoxFuture<'a, ExecutionOutcome> {
            let panic = self.panic.lock().expect("fake lock").clone();
            if let Some((PanicAt::Call, message)) = &panic {
                panic!("{message}");
            }
            if let Some((PanicAt::Drop, message)) = &panic {
                let pending = PanicsWhenDropped(message.clone());
                return Box::pin(async move { match pending.await {} });
            }
            Box::pin(async move {
                if let Some((PanicAt::Poll, message)) = panic {
                    panic!("{message}");
                }
                self.calls.lock().expect("fake lock").push((
                    action.tool().to_owned(),
                    Value::Object(action.arguments().clone()),
                ));
                self.outcomes
                    .lock()
                    .expect("fake lock")
                    .get_mut(action.tool())
                    .and_then(VecDeque::pop_front)
                    .unwrap_or_else(|| ExecutionOutcome::tool_error("unscripted", Value::Null, 0))
            })
        }
    }

    /// A backend that returns scripted bytes in order.
    #[derive(Debug, Default)]
    pub struct ScriptedBackend {
        panic: Mutex<Option<(PanicAt, String)>>,
        panicking_methods: PanickingMethods,
        responses: Mutex<VecDeque<Result<Vec<u8>, BackendError>>>,
        requests: Mutex<Vec<GenerateRequest>>,
        remote: bool,
        transport: Mutex<DecisionTransport>,
        transport_error: Mutex<Option<BackendError>>,
        text_chars: Mutex<VecDeque<Option<u64>>>,
        last_text_chars: Mutex<Option<u64>>,
    }

    impl ScriptedBackend {
        /// A backend returning these decisions as compact JSON.
        pub fn with_decisions(decisions: Vec<Value>) -> Self {
            Self {
                panic: Mutex::new(None),
                panicking_methods: Mutex::default(),
                responses: Mutex::new(
                    decisions
                        .into_iter()
                        .map(|d| Ok(serde_json::to_vec(&d).expect("serializes")))
                        .collect(),
                ),
                requests: Mutex::new(Vec::new()),
                remote: false,
                transport: Mutex::new(DecisionTransport::Envelope),
                transport_error: Mutex::new(None),
                text_chars: Mutex::new(VecDeque::new()),
                last_text_chars: Mutex::new(None),
            }
        }

        /// The same backend in the `tool_calls` transport: the scripted
        /// decisions stand for the mapped envelope of a structured call
        /// (REQ-MDL-009), and the runtime assembles native turns for it.
        pub fn tool_calls(self) -> Self {
            *self.transport.lock().expect("fake lock") = DecisionTransport::ToolCalls;
            self
        }

        /// From now on the transport cannot be settled: the engine refuses a
        /// configured `tool_calls`, or does not answer (REQ-MDL-009).
        pub fn fail_transport(&self, error: BackendError) {
            *self.transport_error.lock().expect("fake lock") = Some(error);
        }

        /// Queues the text length reported beside the next reply's call
        /// (`None` for a reply without text), one entry per reply.
        pub fn push_text_chars(&self, chars: Option<u64>) {
            self.text_chars.lock().expect("fake lock").push_back(chars);
        }

        /// The same backend declaring itself `remote` (REQ-SEC-010 tests).
        pub fn remote(mut self) -> Self {
            self.remote = true;
            self
        }

        /// From now on, `generate` panics with `message` at `at`.
        pub fn panic_on_generate(&self, at: PanicAt, message: &str) {
            *self.panic.lock().expect("fake lock") = Some((at, message.to_owned()));
        }

        /// From now on, the named method (`describe`, `locality` or
        /// `last_usage`) panics with `message`.
        pub fn panic_in(&self, method: &'static str, message: &str) {
            self.panicking_methods
                .lock()
                .expect("fake lock")
                .insert(method, message.to_owned());
        }

        /// No method panics any more.
        pub fn stop_panicking(&self) {
            *self.panic.lock().expect("fake lock") = None;
            self.panicking_methods.lock().expect("fake lock").clear();
        }

        /// Queues raw bytes or a failure.
        pub fn push(&self, response: Result<Vec<u8>, BackendError>) {
            self.responses
                .lock()
                .expect("fake lock")
                .push_back(response);
        }

        /// Every request so far.
        pub fn requests(&self) -> Vec<GenerateRequest> {
            self.requests.lock().expect("fake lock").clone()
        }
    }

    impl ModelBackend for ScriptedBackend {
        fn generate<'a>(
            &'a self,
            request: GenerateRequest,
            _cancel: Cancellation,
        ) -> BoxFuture<'a, Result<Vec<u8>, BackendError>> {
            let panic = self.panic.lock().expect("fake lock").clone();
            if let Some((PanicAt::Call, message)) = &panic {
                panic!("{message}");
            }
            if let Some((PanicAt::Drop, message)) = &panic {
                let pending = PanicsWhenDropped(message.clone());
                return Box::pin(async move { match pending.await {} });
            }
            Box::pin(async move {
                if let Some((PanicAt::Poll, message)) = panic {
                    panic!("{message}");
                }
                self.requests.lock().expect("fake lock").push(request);
                *self.last_text_chars.lock().expect("fake lock") = self
                    .text_chars
                    .lock()
                    .expect("fake lock")
                    .pop_front()
                    .flatten();
                self.responses
                    .lock()
                    .expect("fake lock")
                    .pop_front()
                    .unwrap_or(Err(BackendError::ScriptExhausted))
            })
        }

        fn decision_transport<'a>(
            &'a self,
            _cancel: Cancellation,
        ) -> BoxFuture<'a, Result<DecisionTransport, BackendError>> {
            Box::pin(async move {
                if let Some(error) = self.transport_error.lock().expect("fake lock").clone() {
                    return Err(error);
                }
                Ok(*self.transport.lock().expect("fake lock"))
            })
        }

        fn last_text_chars(&self) -> Option<u64> {
            *self.last_text_chars.lock().expect("fake lock")
        }

        fn locality(&self) -> Locality {
            panic_if_set(&self.panicking_methods, "locality");
            if self.remote {
                Locality::Remote
            } else {
                Locality::Local
            }
        }

        fn last_usage(&self) -> Option<Usage> {
            panic_if_set(&self.panicking_methods, "last_usage");
            None
        }

        fn describe(&self) -> BackendDescriptor {
            panic_if_set(&self.panicking_methods, "describe");
            BackendDescriptor {
                kind: "scripted".into(),
                model: None,
                locality: if self.remote {
                    Locality::Remote
                } else {
                    Locality::Local
                },
            }
        }
    }

    /// A workspace provider that materializes nothing and serves one tool provider.
    pub struct FakeWorkspaces {
        tools: Arc<dyn ToolProvider>,
        remaining: Mutex<Option<u64>>,
        snapshots: Mutex<Vec<SnapshotRecord>>,
        restores: Mutex<Vec<SnapshotId>>,
    }

    impl FakeWorkspaces {
        /// Serves `tools` for every task.
        pub fn new(tools: Arc<dyn ToolProvider>) -> Self {
            Self {
                tools,
                remaining: Mutex::new(None),
                snapshots: Mutex::new(Vec::new()),
                restores: Mutex::new(Vec::new()),
            }
        }

        /// Sets the remaining workspace bytes reported.
        pub fn set_remaining(&self, bytes: Option<u64>) {
            *self.remaining.lock().expect("fake lock") = bytes;
        }
    }

    impl WorkspaceProvider for FakeWorkspaces {
        fn materialize(
            &self,
            _task_id: TaskId,
            scope: &ScopeSpec,
        ) -> Result<WorkspaceInfo, WorkspaceError> {
            Ok(WorkspaceInfo {
                root: format!("/fake/workspaces/{}", scope.root.trim_start_matches('/')),
                case_mode: CaseMode::Sensitive,
                bytes: 0,
                files: 0,
                symlinks_omitted: 0,
            })
        }

        fn open(&self, task_id: TaskId) -> Result<WorkspaceInfo, WorkspaceError> {
            self.materialize(
                task_id,
                &ScopeSpec {
                    root: "fake".into(),
                    include: vec![],
                },
            )
        }

        fn tools(&self, _task_id: TaskId) -> Result<Arc<dyn ToolProvider>, WorkspaceError> {
            Ok(Arc::clone(&self.tools))
        }

        fn remaining_bytes(&self, _task_id: TaskId) -> Result<Option<u64>, WorkspaceError> {
            Ok(*self.remaining.lock().expect("fake lock"))
        }

        fn snapshot(
            &self,
            task_id: TaskId,
            snapshot_id: SnapshotId,
            reason: SnapshotReason,
            action_id: Option<ActionId>,
            previous: Option<&SnapshotRecord>,
            now: crate::clock::Instant,
        ) -> Result<PreparedSnapshot, WorkspaceError> {
            let manifest_hash = Manifest::default().hash();
            if let Some(p) = previous
                && p.manifest_hash == manifest_hash
            {
                return Ok(PreparedSnapshot {
                    record: p.clone(),
                    objects: vec![],
                    reused: true,
                });
            }
            let record = SnapshotRecord {
                snapshot_id,
                task_id,
                reason,
                manifest_hash,
                bytes: 0,
                files: 0,
                created_at: now,
                action_id,
                project_root: None,
            };
            self.snapshots
                .lock()
                .expect("fake lock")
                .push(record.clone());
            Ok(PreparedSnapshot {
                record,
                objects: vec![],
                reused: false,
            })
        }

        fn seal(&self, _snapshot_id: SnapshotId) -> Result<(), WorkspaceError> {
            Ok(())
        }

        fn restore(
            &self,
            task_id: TaskId,
            snapshot: &SnapshotRecord,
        ) -> Result<RestoreReport, WorkspaceError> {
            if snapshot.task_id != task_id && snapshot.reason != SnapshotReason::Backup {
                return Err(WorkspaceError::NotFound {
                    what: "snapshot".into(),
                });
            }
            self.restores
                .lock()
                .expect("fake lock")
                .push(snapshot.snapshot_id);
            Ok(RestoreReport::default())
        }

        fn manifest(&self, _snapshot: &SnapshotRecord) -> Result<Manifest, WorkspaceError> {
            Ok(Manifest::default())
        }

        fn materialize_from(
            &self,
            task_id: TaskId,
            snapshot_id: SnapshotId,
            from: &SnapshotRecord,
            scope: &ScopeSpec,
            now: crate::clock::Instant,
        ) -> Result<(WorkspaceInfo, PreparedSnapshot), WorkspaceError> {
            let info = self.materialize(task_id, scope)?;
            let record = SnapshotRecord {
                snapshot_id,
                task_id,
                reason: SnapshotReason::Fork,
                manifest_hash: from.manifest_hash.clone(),
                bytes: from.bytes,
                files: from.files,
                created_at: now,
                action_id: None,
                project_root: None,
            };
            self.snapshots
                .lock()
                .expect("fake lock")
                .push(record.clone());
            Ok((
                info,
                PreparedSnapshot {
                    record,
                    objects: vec![],
                    reused: false,
                },
            ))
        }

        fn promotion_plan(
            &self,
            _task_id: TaskId,
            _base: &SnapshotRecord,
            _project_root: &str,
        ) -> Result<PromotionPlan, WorkspaceError> {
            Err(WorkspaceError::Refused {
                reason: "no promotion in the fake".into(),
            })
        }

        fn promote(
            &self,
            _task_id: TaskId,
            _plan: &PromotionPlan,
            _project_root: &str,
            _backup_id: SnapshotId,
            _promotion_id: PromotionId,
            _now: crate::clock::Instant,
        ) -> Result<(PreparedSnapshot, Promotion), WorkspaceError> {
            Err(WorkspaceError::Refused {
                reason: "no promotion in the fake".into(),
            })
        }

        fn revert_promotion(
            &self,
            _promotion: &Promotion,
            _backup: &SnapshotRecord,
        ) -> Result<RestoreReport, WorkspaceError> {
            Err(WorkspaceError::NotFound {
                what: "promotion".into(),
            })
        }

        fn tree_bytes(&self, _task_id: TaskId) -> Result<u64, WorkspaceError> {
            Ok(0)
        }

        fn delete_files(&self, task_id: TaskId, _rows: &DeletedRows) -> Result<(), WorkspaceError> {
            self.snapshots
                .lock()
                .expect("fake lock")
                .retain(|s| s.task_id != task_id);
            Ok(())
        }
    }

    impl FakeWorkspaces {
        /// Snapshots taken, in order.
        pub fn snapshots(&self) -> Vec<SnapshotRecord> {
            self.snapshots.lock().expect("fake lock").clone()
        }

        /// Restores performed, in order.
        pub fn restores(&self) -> Vec<SnapshotId> {
            self.restores.lock().expect("fake lock").clone()
        }
    }

    /// An in-memory persistence with the same write-ordering rules as the
    /// SQLite store, for deterministic runtime tests (REQ-ARCH-006).
    #[derive(Default)]
    pub struct FakePersistence {
        inner: Mutex<FakeInner>,
    }

    #[derive(Default)]
    struct FakeInner {
        tasks: Vec<Task>,
        steps: Vec<Step>,
        actions: Vec<Action>,
        executions: Vec<Execution>,
        observations: Vec<Observation>,
        approvals: Vec<ApprovalRequest>,
        questions: Vec<Question>,
        grants: Vec<Grant>,
        plans: Vec<PlanVersion>,
        events: Vec<EventRecord>,
        blobs: Vec<(BlobId, Vec<u8>)>,
        snapshots: Vec<SnapshotRecord>,
        objects: std::collections::BTreeMap<crate::hash::Hash, (u64, u64)>,
        workspace_states: std::collections::BTreeMap<TaskId, WorkspaceState>,
        promotions: Vec<Promotion>,
        /// Names of the write methods called, in order (for ordering tests).
        pub writes: Vec<&'static str>,
    }

    impl FakePersistence {
        /// The write methods called so far, in order.
        pub fn writes(&self) -> Vec<&'static str> {
            self.inner.lock().expect("fake lock").writes.clone()
        }

        /// Every event so far.
        pub fn events(&self) -> Vec<EventRecord> {
            self.inner.lock().expect("fake lock").events.clone()
        }

        fn write<R>(
            &self,
            name: &'static str,
            f: impl FnOnce(&mut FakeInner) -> Result<R, PersistenceError>,
        ) -> Result<R, PersistenceError> {
            let mut inner = self.inner.lock().expect("fake lock");
            let r = f(&mut inner)?;
            inner.writes.push(name);
            Ok(r)
        }
    }

    fn upsert<T: Clone>(items: &mut Vec<T>, item: &T, same: impl Fn(&T) -> bool) {
        match items.iter_mut().find(|x| same(x)) {
            Some(slot) => *slot = item.clone(),
            None => items.push(item.clone()),
        }
    }

    fn expect_status(
        inner: &FakeInner,
        action: &Action,
        allowed: &[&str],
        line: &str,
    ) -> Result<(), PersistenceError> {
        match inner
            .actions
            .iter()
            .find(|a| a.action_id == action.action_id)
        {
            Some(stored) if allowed.contains(&stored.status.name()) => Ok(()),
            Some(stored) => Err(PersistenceError::Ordering {
                detail: format!(
                    "{line} requires status in {allowed:?}, found {}",
                    stored.status.name()
                ),
            }),
            None => Err(PersistenceError::Ordering {
                detail: format!("{line} requires a recorded action"),
            }),
        }
    }

    impl Persistence for FakePersistence {
        fn create_task(
            &self,
            task: &Task,
            grants: &[Grant],
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("create_task", |i| {
                i.tasks.push(task.clone());
                i.grants.extend(grants.iter().cloned());
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn load_task(&self, task_id: TaskId) -> Result<Option<Task>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .tasks
                .iter()
                .find(|t| t.task_id == task_id)
                .cloned())
        }

        fn list_tasks(&self) -> Result<Vec<TaskSummary>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .tasks
                .iter()
                .map(|t| TaskSummary {
                    task_id: t.task_id,
                    goal: t.goal.clone(),
                    state: t.state.name().to_owned(),
                    created_at: t.created_at,
                    updated_at: t.updated_at,
                })
                .collect())
        }

        fn tasks_in_state(&self, state: &str) -> Result<Vec<TaskId>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .tasks
                .iter()
                .filter(|t| t.state.name() == state)
                .map(|t| t.task_id)
                .collect())
        }

        fn record_task_transition(
            &self,
            task: &Task,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("task_transition", |i| {
                upsert(&mut i.tasks, task, |t| t.task_id == task.task_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_step_started(
            &self,
            step: &Step,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("step_started", |i| {
                upsert(&mut i.steps, step, |s| s.step_id == step.step_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_proposed(
            &self,
            step: &Step,
            action: &Action,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("proposed", |i| {
                upsert(&mut i.steps, step, |s| s.step_id == step.step_id);
                i.actions.push(action.clone());
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_validation(
            &self,
            step: &Step,
            action: Option<&Action>,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("validation", |i| {
                upsert(&mut i.steps, step, |s| s.step_id == step.step_id);
                if let Some(action) = action {
                    expect_status(i, action, &["proposed"], "validation")?;
                    upsert(&mut i.actions, action, |a| a.action_id == action.action_id);
                }
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_approval_request(
            &self,
            task: &Task,
            action: &Action,
            approval: &ApprovalRequest,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("approval_request", |i| {
                expect_status(
                    i,
                    action,
                    &["proposed", "waiting_approval"],
                    "approval_request",
                )?;
                upsert(&mut i.actions, action, |a| a.action_id == action.action_id);
                upsert(&mut i.approvals, approval, |a| {
                    a.approval_id == approval.approval_id
                });
                upsert(&mut i.tasks, task, |t| t.task_id == task.task_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_approval_decision(
            &self,
            task: &Task,
            approval: &ApprovalRequest,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("approval_decision", |i| {
                upsert(&mut i.approvals, approval, |a| {
                    a.approval_id == approval.approval_id
                });
                upsert(&mut i.tasks, task, |t| t.task_id == task.task_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_authorized(
            &self,
            action: &Action,
            consumed: Option<&ApprovalRequest>,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("authorized", |i| {
                expect_status(
                    i,
                    action,
                    &["proposed", "waiting_approval", "failed"],
                    "authorized",
                )?;
                upsert(&mut i.actions, action, |a| a.action_id == action.action_id);
                if let Some(approval) = consumed {
                    upsert(&mut i.approvals, approval, |a| {
                        a.approval_id == approval.approval_id
                    });
                }
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_running(
            &self,
            action: &Action,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("running", |i| {
                expect_status(i, action, &["authorized"], "running")?;
                upsert(&mut i.actions, action, |a| a.action_id == action.action_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_execution(
            &self,
            execution: &Execution,
            observation: &Observation,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("execution", |i| {
                i.executions.push(execution.clone());
                i.observations.push(observation.clone());
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_action_terminal(
            &self,
            action: &Action,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("action_terminal", |i| {
                let allowed: &[&str] = match action.status {
                    crate::action::ActionStatus::Rejected { .. } => {
                        &["proposed", "waiting_approval"]
                    }
                    crate::action::ActionStatus::Cancelled => {
                        &["proposed", "waiting_approval", "authorized", "running"]
                    }
                    _ => &["running"],
                };
                expect_status(i, action, allowed, "action_terminal")?;
                upsert(&mut i.actions, action, |a| a.action_id == action.action_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_step_outcome(
            &self,
            step: &Step,
            task: &Task,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("step_outcome", |i| {
                upsert(&mut i.steps, step, |s| s.step_id == step.step_id);
                upsert(&mut i.tasks, task, |t| t.task_id == task.task_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_question(
            &self,
            task: &Task,
            question: &Question,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("question", |i| {
                upsert(&mut i.questions, question, |q| {
                    q.question_id == question.question_id
                });
                upsert(&mut i.tasks, task, |t| t.task_id == task.task_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_question_update(
            &self,
            task: &Task,
            question: &Question,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.record_question(task, question, event)
        }

        fn record_grant(&self, grant: &Grant, event: &EventRecord) -> Result<(), PersistenceError> {
            self.write("grant", |i| {
                i.grants.push(grant.clone());
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_grant_status(
            &self,
            grant: &Grant,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("grant_status", |i| {
                upsert(&mut i.grants, grant, |g| g.grant_id == grant.grant_id);
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn record_plan(
            &self,
            plan: &PlanVersion,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("plan", |i| {
                i.plans.push(plan.clone());
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn load_grants(&self, task_id: TaskId) -> Result<Vec<Grant>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .grants
                .iter()
                .filter(|g| g.task_id == task_id)
                .cloned()
                .collect())
        }

        fn load_steps(&self, task_id: TaskId) -> Result<Vec<Step>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .steps
                .iter()
                .filter(|s| s.task_id == task_id)
                .cloned()
                .collect())
        }

        fn load_actions(&self, task_id: TaskId) -> Result<Vec<Action>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .actions
                .iter()
                .filter(|a| a.task_id == task_id)
                .cloned()
                .collect())
        }

        fn load_running_action(&self, task_id: TaskId) -> Result<Option<Action>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .actions
                .iter()
                .rev()
                .find(|a| a.task_id == task_id && a.status == crate::action::ActionStatus::Running)
                .cloned())
        }

        fn load_observations(&self, task_id: TaskId) -> Result<Vec<Observation>, PersistenceError> {
            let inner = self.inner.lock().expect("fake lock");
            let ids: Vec<ActionId> = inner
                .actions
                .iter()
                .filter(|a| a.task_id == task_id)
                .map(|a| a.action_id)
                .collect();
            Ok(inner
                .observations
                .iter()
                .filter(|o| ids.contains(&o.action_id))
                .cloned()
                .collect())
        }

        fn load_executions(&self, action_id: ActionId) -> Result<Vec<Execution>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .executions
                .iter()
                .filter(|e| e.action_id == action_id)
                .cloned()
                .collect())
        }

        fn load_approvals(
            &self,
            task_id: TaskId,
        ) -> Result<Vec<ApprovalRequest>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .approvals
                .iter()
                .filter(|a| a.task_id == task_id)
                .cloned()
                .collect())
        }

        fn load_pending_approval(
            &self,
            task_id: TaskId,
        ) -> Result<Option<ApprovalRequest>, PersistenceError> {
            use crate::approval::ApprovalStatus;
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .approvals
                .iter()
                .rev()
                .find(|a| {
                    a.task_id == task_id
                        && matches!(a.status, ApprovalStatus::Pending | ApprovalStatus::Approved)
                })
                .cloned())
        }

        fn load_pending_question(
            &self,
            task_id: TaskId,
        ) -> Result<Option<Question>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .questions
                .iter()
                .rev()
                .find(|q| {
                    q.task_id == task_id && q.status == crate::question::QuestionStatus::Pending
                })
                .cloned())
        }

        fn load_questions(&self, task_id: TaskId) -> Result<Vec<Question>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .questions
                .iter()
                .filter(|q| q.task_id == task_id)
                .cloned()
                .collect())
        }

        fn load_plan(&self, task_id: TaskId) -> Result<Option<PlanVersion>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .plans
                .iter()
                .filter(|p| p.task_id == task_id)
                .max_by_key(|p| p.version)
                .cloned())
        }

        fn append_event(&self, event: &EventRecord) -> Result<(), PersistenceError> {
            self.write("event", |i| {
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn metrics(&self, task_id: Option<TaskId>) -> Result<Metrics, PersistenceError> {
            let inner = self.inner.lock().expect("fake lock");
            let mut m = Metrics::default();
            for e in inner
                .events
                .iter()
                .filter(|e| task_id.is_none() || e.task_id == task_id)
            {
                *m.events_by_name
                    .entry(e.event.name().to_owned())
                    .or_default() += 1;
            }
            for t in inner
                .tasks
                .iter()
                .filter(|t| task_id.is_none_or(|id| t.task_id == id))
            {
                *m.tasks_by_state
                    .entry(t.state.name().to_owned())
                    .or_default() += 1;
            }
            let mut sums = ContextSums::default();
            for s in inner
                .steps
                .iter()
                .filter(|s| task_id.is_none_or(|id| s.task_id == id))
            {
                sums.add(s);
            }
            m.context = sums.finish();
            Ok(m)
        }

        fn events_since(
            &self,
            position: u64,
            task_id: Option<TaskId>,
            limit: usize,
        ) -> Result<Vec<PositionedEvent>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .events
                .iter()
                .enumerate()
                .map(|(i, e)| (i as u64 + 1, e))
                .filter(|(p, e)| *p > position && (task_id.is_none() || e.task_id == task_id))
                .take(limit)
                .map(|(position, record)| PositionedEvent {
                    position,
                    record: record.clone(),
                })
                .collect())
        }

        fn put_blob(&self, _task_id: TaskId, bytes: &[u8]) -> Result<BlobId, PersistenceError> {
            let id = BlobId::from_uuid(uuid::Uuid::from_u128(
                self.inner.lock().expect("fake lock").blobs.len() as u128 + 1,
            ));
            self.inner
                .lock()
                .expect("fake lock")
                .blobs
                .push((id, bytes.to_vec()));
            Ok(id)
        }

        fn record_snapshot(
            &self,
            snapshot: &SnapshotRecord,
            objects: &[(crate::hash::Hash, u64)],
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("snapshot", |i| {
                i.snapshots.push(snapshot.clone());
                for (hash, size) in objects {
                    let e = i.objects.entry(hash.clone()).or_insert((*size, 0));
                    e.1 += 1;
                }
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn load_snapshots(&self, task_id: TaskId) -> Result<Vec<SnapshotRecord>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .snapshots
                .iter()
                .filter(|s| s.task_id == task_id)
                .cloned()
                .collect())
        }

        fn load_snapshot(
            &self,
            snapshot_id: SnapshotId,
        ) -> Result<Option<SnapshotRecord>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .snapshots
                .iter()
                .find(|s| s.snapshot_id == snapshot_id)
                .cloned())
        }

        fn record_workspace_state(
            &self,
            task_id: TaskId,
            state: WorkspaceState,
            event: Option<&EventRecord>,
        ) -> Result<(), PersistenceError> {
            self.write("workspace_state", |i| {
                i.workspace_states.insert(task_id, state);
                if let Some(e) = event {
                    i.events.push(e.clone());
                }
                Ok(())
            })
        }

        fn load_workspace_state(
            &self,
            task_id: TaskId,
        ) -> Result<Option<WorkspaceState>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .workspace_states
                .get(&task_id)
                .copied())
        }

        fn record_promotion(
            &self,
            promotion: &Promotion,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("promotion", |i| {
                upsert(&mut i.promotions, promotion, |p| {
                    p.promotion_id == promotion.promotion_id
                });
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn load_promotions(&self, task_id: TaskId) -> Result<Vec<Promotion>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .promotions
                .iter()
                .filter(|p| p.task_id == task_id)
                .cloned()
                .collect())
        }

        fn object_references(
            &self,
            hashes: &[crate::hash::Hash],
        ) -> Result<Vec<(crate::hash::Hash, u64, u64)>, PersistenceError> {
            let i = self.inner.lock().expect("fake lock");
            Ok(hashes
                .iter()
                .filter_map(|h| {
                    i.objects
                        .get(h)
                        .map(|(size, refs)| (h.clone(), *size, *refs))
                })
                .collect())
        }

        fn delete_task_data(
            &self,
            task_id: TaskId,
            audit: bool,
            event: &EventRecord,
        ) -> Result<DeletedRows, PersistenceError> {
            self.write("delete_task_data", |i| {
                let mut out = DeletedRows::default();
                let removed: Vec<SnapshotRecord> = i
                    .snapshots
                    .iter()
                    .filter(|s| s.task_id == task_id)
                    .cloned()
                    .collect();
                i.snapshots.retain(|s| s.task_id != task_id);
                for s in &removed {
                    out.snapshots.push(s.snapshot_id);
                }
                // The fake has no manifests to count from; every object referenced once per snapshot is approximated as released.
                let zero: Vec<crate::hash::Hash> = i
                    .objects
                    .iter_mut()
                    .filter_map(|(h, (_, refs))| {
                        if *refs > 0 {
                            *refs -= removed.len().min(*refs as usize) as u64;
                        }
                        (*refs == 0).then(|| h.clone())
                    })
                    .collect();
                for h in &zero {
                    i.objects.remove(h);
                }
                out.objects_at_zero = zero;
                i.promotions.retain(|p| p.task_id != task_id);
                i.workspace_states.insert(task_id, WorkspaceState::Deleted);
                if audit {
                    i.events.retain(|e| e.task_id != Some(task_id));
                    i.tasks.retain(|t| t.task_id != task_id);
                    i.steps.retain(|s| s.task_id != task_id);
                    i.actions.retain(|a| a.task_id != task_id);
                    i.approvals.retain(|a| a.task_id != task_id);
                    i.questions.retain(|q| q.task_id != task_id);
                    i.grants.retain(|g| g.task_id != task_id);
                    i.plans.retain(|p| p.task_id != task_id);
                } else {
                    i.events.push(event.clone());
                }
                Ok(out)
            })
        }

        fn blob_bytes(&self, _task_id: TaskId) -> Result<u64, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .blobs
                .iter()
                .map(|(_, b)| b.len() as u64)
                .sum())
        }

        fn get_blob(&self, blob_id: BlobId) -> Result<Vec<u8>, PersistenceError> {
            self.inner
                .lock()
                .expect("fake lock")
                .blobs
                .iter()
                .find(|(id, _)| *id == blob_id)
                .map(|(_, b)| b.clone())
                .ok_or(PersistenceError::NotFound {
                    what: "blob".into(),
                })
        }
    }
}

/// A persistence failure. The runtime fails closed on every variant (REQ-SEC-003).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum PersistenceError {
    /// The store could not be opened or is locked by another runtime.
    #[error("store locked by another runtime")]
    Locked,
    /// The database schema is newer than this runtime understands.
    #[error("database schema version {found} is newer than the supported {supported}")]
    NewerSchema {
        /// Found in the database.
        found: u32,
        /// Supported by this runtime.
        supported: u32,
    },
    /// An integrity or consistency rule failed.
    #[error("integrity violation: {rule}: {detail}")]
    Integrity {
        /// The rule (`integrity_check`, `CONS-01` …).
        rule: String,
        /// What was found.
        detail: String,
    },
    /// A record was not found.
    #[error("not found: {what}")]
    NotFound {
        /// What.
        what: String,
    },
    /// A write would violate the write ordering (for example running before authorized).
    #[error("write ordering violated: {detail}")]
    Ordering {
        /// What.
        detail: String,
    },
    /// Storage failed.
    #[error("storage failure: {detail}")]
    Storage {
        /// A fixed-vocabulary detail; never file content.
        detail: String,
    },
}

/// A row of `list_tasks`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSummary {
    /// Identifier.
    pub task_id: TaskId,
    /// Goal.
    pub goal: String,
    /// State name.
    pub state: String,
    /// Created.
    pub created_at: crate::clock::Instant,
    /// Updated.
    pub updated_at: crate::clock::Instant,
}

/// An event with its position in the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionedEvent {
    /// Monotonic position.
    pub position: u64,
    /// The event.
    pub record: EventRecord,
}

/// Durable storage of tasks and everything they own (external design §16;
/// REQ-PERS-001 … 012). Every method is one transaction that also appends
/// the event it is given, and returns only after the commit (REQ-PERS-003).
/// The methods `record_proposed` … `record_step_outcome` are the eight
/// ordered lines of external design §16.2 (REQ-PERS-004).
pub trait Persistence: Send + Sync {
    /// Creates a task with its initial grants (line 0).
    fn create_task(
        &self,
        task: &Task,
        grants: &[Grant],
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// Loads a task.
    fn load_task(&self, task_id: TaskId) -> Result<Option<Task>, PersistenceError>;
    /// Lists tasks, newest first.
    fn list_tasks(&self) -> Result<Vec<TaskSummary>, PersistenceError>;
    /// Tasks whose state has this name.
    fn tasks_in_state(&self, state: &str) -> Result<Vec<TaskId>, PersistenceError>;
    /// Persists a task transition outside a step (pause, resume, cancel, halt, recovery).
    fn record_task_transition(
        &self,
        task: &Task,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;

    /// A step started.
    fn record_step_started(&self, step: &Step, event: &EventRecord)
    -> Result<(), PersistenceError>;
    /// Line 1: the proposal with its hash.
    fn record_proposed(
        &self,
        step: &Step,
        action: &Action,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// Line 2: the validation result (a rejection, or the action pending approval or authorized).
    fn record_validation(
        &self,
        step: &Step,
        action: Option<&Action>,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// Line 3: an approval request, with the task now waiting.
    fn record_approval_request(
        &self,
        task: &Task,
        action: &Action,
        approval: &ApprovalRequest,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// Line 3: the user's decision, with the task's new state.
    fn record_approval_decision(
        &self,
        task: &Task,
        approval: &ApprovalRequest,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// Line 4: authorized, together with the consumed approval when one was required (one transition).
    fn record_authorized(
        &self,
        action: &Action,
        consumed: Option<&ApprovalRequest>,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// Line 5: the running intent; execution may start only after this returns.
    fn record_running(&self, action: &Action, event: &EventRecord) -> Result<(), PersistenceError>;
    /// Line 6: the execution record and its observation.
    fn record_execution(
        &self,
        execution: &Execution,
        observation: &Observation,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// Line 7: the action's terminal status.
    fn record_action_terminal(
        &self,
        action: &Action,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// Line 8: the step's outcome with the task's counters and state.
    fn record_step_outcome(
        &self,
        step: &Step,
        task: &Task,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;

    /// A question was asked, with the task now waiting.
    fn record_question(
        &self,
        task: &Task,
        question: &Question,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// A question was answered or expired, with the task's new state.
    fn record_question_update(
        &self,
        task: &Task,
        question: &Question,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// A grant was issued.
    fn record_grant(&self, grant: &Grant, event: &EventRecord) -> Result<(), PersistenceError>;
    /// A grant changed status.
    fn record_grant_status(
        &self,
        grant: &Grant,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// A plan version was created.
    fn record_plan(&self, plan: &PlanVersion, event: &EventRecord) -> Result<(), PersistenceError>;

    /// The task's grants.
    fn load_grants(&self, task_id: TaskId) -> Result<Vec<Grant>, PersistenceError>;
    /// The task's steps in order.
    fn load_steps(&self, task_id: TaskId) -> Result<Vec<Step>, PersistenceError>;
    /// The task's actions in order.
    fn load_actions(&self, task_id: TaskId) -> Result<Vec<Action>, PersistenceError>;
    /// The task's action in status `running`, if any.
    fn load_running_action(&self, task_id: TaskId) -> Result<Option<Action>, PersistenceError>;
    /// The task's observations in order.
    fn load_observations(&self, task_id: TaskId) -> Result<Vec<Observation>, PersistenceError>;
    /// The executions of an action.
    fn load_executions(&self, action_id: ActionId) -> Result<Vec<Execution>, PersistenceError>;
    /// Every approval request of the task, oldest first.
    fn load_approvals(&self, task_id: TaskId) -> Result<Vec<ApprovalRequest>, PersistenceError>;
    /// The task's pending approval, if any.
    fn load_pending_approval(
        &self,
        task_id: TaskId,
    ) -> Result<Option<ApprovalRequest>, PersistenceError>;
    /// The task's pending question, if any.
    fn load_pending_question(&self, task_id: TaskId) -> Result<Option<Question>, PersistenceError>;
    /// Every question of the task, oldest first.
    fn load_questions(&self, task_id: TaskId) -> Result<Vec<Question>, PersistenceError>;
    /// The task's latest plan version, if any.
    fn load_plan(&self, task_id: TaskId) -> Result<Option<PlanVersion>, PersistenceError>;

    /// Records a snapshot whose files are durable, and the objects it references
    /// (counts incremented in the same transaction) (REQ-WS-004, REQ-PERS-009).
    fn record_snapshot(
        &self,
        snapshot: &SnapshotRecord,
        objects: &[(crate::hash::Hash, u64)],
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// The task's snapshots, oldest first.
    fn load_snapshots(&self, task_id: TaskId) -> Result<Vec<SnapshotRecord>, PersistenceError>;
    /// One snapshot.
    fn load_snapshot(
        &self,
        snapshot_id: SnapshotId,
    ) -> Result<Option<SnapshotRecord>, PersistenceError>;
    /// The workspace's lifecycle state (REQ-WS-009).
    fn record_workspace_state(
        &self,
        task_id: TaskId,
        state: WorkspaceState,
        event: Option<&EventRecord>,
    ) -> Result<(), PersistenceError>;
    /// The workspace's recorded state, if any.
    fn load_workspace_state(
        &self,
        task_id: TaskId,
    ) -> Result<Option<WorkspaceState>, PersistenceError>;
    /// A promotion applied (REQ-WS-007).
    fn record_promotion(
        &self,
        promotion: &Promotion,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;
    /// The task's promotions, oldest first.
    fn load_promotions(&self, task_id: TaskId) -> Result<Vec<Promotion>, PersistenceError>;
    /// How many manifests reference each of these objects, and their sizes.
    fn object_references(
        &self,
        hashes: &[crate::hash::Hash],
    ) -> Result<Vec<(crate::hash::Hash, u64, u64)>, PersistenceError>;
    /// Bytes of the task's blobs.
    fn blob_bytes(&self, task_id: TaskId) -> Result<u64, PersistenceError>;
    /// Removes the task's snapshot, workspace, promotion and blob rows (and,
    /// with `audit`, its events and records), decrementing the objects;
    /// returns the object hashes that reached zero and the blob ids removed.
    fn delete_task_data(
        &self,
        task_id: TaskId,
        audit: bool,
        event: &EventRecord,
    ) -> Result<DeletedRows, PersistenceError>;
    /// Appends an event that belongs to no record transition (runtime start and stop, tool pinning, recovery dispositions).
    fn append_event(&self, event: &EventRecord) -> Result<(), PersistenceError>;
    /// Metrics over all tasks, or one task.
    fn metrics(&self, task_id: Option<TaskId>) -> Result<Metrics, PersistenceError>;
    /// Events after `position`, oldest first, at most `limit`.
    fn events_since(
        &self,
        position: u64,
        task_id: Option<TaskId>,
        limit: usize,
    ) -> Result<Vec<PositionedEvent>, PersistenceError>;
    /// Stores a bounded blob and returns its id (file first, then the row).
    fn put_blob(&self, task_id: TaskId, bytes: &[u8]) -> Result<BlobId, PersistenceError>;
    /// Reads a blob.
    fn get_blob(&self, blob_id: BlobId) -> Result<Vec<u8>, PersistenceError>;
}
