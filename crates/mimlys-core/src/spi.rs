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
    Manifest, Promotion, PromotionPlan, RestoreReport, RevertPlan, SnapshotReason, SnapshotRecord,
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

/// A slot or a gate that is held: dropping it returns it (RFC 013 decisions 5 and 7). The
/// runtime keeps it for the length of an execution or a model request and knows nothing else of
/// it.
pub struct Held(#[allow(dead_code)] Box<dyn std::any::Any + Send + Sync>);

impl Held {
    /// Wraps a guard whose drop returns what it holds.
    pub fn new<T: std::any::Any + Send + Sync>(guard: T) -> Self {
        Self(Box::new(guard))
    }
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Held")
    }
}

/// What a wait for a slot or a gate reports as it changes (external design §19.3). The runtime
/// records `task.admission_changed` for each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitChange {
    /// No slot is free: the wait begins.
    Waiting {
        /// Slots taken now, which is the limit.
        slots_taken: u32,
        /// The processes that hold them.
        holders: Vec<u32>,
    },
    /// The wait ended with a slot.
    Admitted {
        /// Slots taken now, this one included.
        slots_taken: u32,
        /// How long the wait lasted, in milliseconds.
        waited_ms: u64,
    },
    /// The registry cannot be read or written within its bound (LIM-O11): the execution goes on
    /// as if alone, and the runtime says so.
    Unadmitted {
        /// Why, in the registry's words.
        reason: String,
    },
}

/// What a wait for a slot or a gate is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionRequest {
    /// The task.
    pub task_id: TaskId,
    /// The task's deadline in milliseconds from now: the wait ends when it passes (the deadline
    /// keeps counting, RFC 013 decision 5).
    pub remaining_ms: i64,
}

/// How a wait for a slot or a gate ended.
#[derive(Debug)]
pub enum AdmissionOutcome {
    /// Admitted. `held` is `None` when the registry could not be used (the fail-open: the
    /// execution goes on unadmitted, and the change `Unadmitted` was reported).
    Admitted {
        /// What to hold until the execution or the request ends.
        held: Option<Held>,
        /// How long the wait lasted, in milliseconds (zero when it did not wait).
        waited_ms: u64,
    },
    /// The first stop request ended the wait: nothing is held, and nothing was sent.
    Stopped {
        /// How long the wait lasted, in milliseconds.
        waited_ms: u64,
    },
    /// The task's deadline passed during the wait: nothing is held.
    DeadlinePassed {
        /// How long the wait lasted, in milliseconds.
        waited_ms: u64,
    },
}

/// A callback that records what a wait reports; an error is the store's.
pub type WaitObserver<'a> = &'a (dyn Fn(WaitChange) -> Result<(), PersistenceError> + Send + Sync);

/// Admission across the runtimes of one user (RFC 013): the same interface serves the execution
/// slots (decision 5) and the engine gate (decision 7). The implementation owns the registry, the
/// waiting and the bound on its own operations (LIM-O11); the runtime owns the order against the
/// record of an action and the events.
pub trait Admission: Send + Sync {
    /// Takes a slot, or the gate, waiting for one. The wait ends at the first stop request
    /// (`stop`, the weakest one, which the two stronger requests imply), at the task's deadline,
    /// or when one is free. `changed` is called as the wait changes, in order, and an error from
    /// it ends the wait with that error. A slot is never held when the outcome is `Stopped` or
    /// `DeadlinePassed`.
    fn admit<'a>(
        &'a self,
        request: AdmissionRequest,
        stop: Cancellation,
        changed: WaitObserver<'a>,
    ) -> BoxFuture<'a, Result<AdmissionOutcome, PersistenceError>>;
}

/// What a tool provider receives beside the action.
#[derive(Clone, Debug)]
pub struct ExecutionContext {
    /// Effective deadline in milliseconds (REQ-TOOL-006).
    pub deadline_ms: i64,
    /// Cancellation flag (REQ-TOOL-007). For an action that cannot be undone it
    /// is the second stop signal's token: it is not set by the first.
    pub cancel: Cancellation,
    /// The start gate's token (REQ-CLI-005, external design §19.2): the weakest
    /// stop request, which the two stronger ones imply, so a first stop signal
    /// sets it. For a wait made **before anything is sent** (an MCP server's pin
    /// check, and the wait for its server to be started again before the call):
    /// a provider that stops at it has made no call, and reports it with
    /// [`ExecutionOutcome::not_started`], which the loop records as the action
    /// not begun, never as a failure of the tool or an unknown outcome. A wait
    /// made after a request was sent observes `cancel`, not this.
    pub start_gate: Cancellation,
    /// The runtime's redactor (REQ-CTX-007). A provider that cuts text redacts
    /// the whole text first, so that no part of a secret is left across the cut;
    /// the runtime redacts what the provider returns all the same.
    pub redactor: crate::context::Redactor,
}

/// What a tool provider returns (external design §7.4, §8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionOutcome {
    /// How the execution ended.
    pub termination: Termination,
    /// The failure the provider itself classified, if any; otherwise the
    /// loop derives it from the termination.
    pub failure: Option<FailureCause>,
    /// The structured result, as the model reads it.
    pub payload: Value,
    /// The whole result, when `payload` is a cut of it (handoff 009i: a result
    /// is bounded in total for the model, and kept whole, within its other
    /// bounds, for the user). It is kept in the observation and never shown to
    /// the model. `None` when nothing was cut: the payload is then the whole.
    pub full_payload: Option<Value>,
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
            full_payload: None,
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
            full_payload: None,
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
            full_payload: None,
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
            full_payload: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
            notices: Vec::new(),
            metrics: ExecutionMetrics {
                wall_ms,
                ..ExecutionMetrics::default()
            },
        }
    }

    /// An outcome that stopped at the start gate's token
    /// ([`ExecutionContext::start_gate`]) in a wait made before anything was
    /// sent: nothing was sent, so nothing is unknown and nothing failed. The
    /// loop records the action as it records one that never began
    /// (`stopped_before_start`) and the step ends `cancelled`, which the caller
    /// turns into a pause. It is a cancelled outcome carrying the notice, so
    /// that [`ExecutionOutcome::is_not_started`] is the one place that tells it
    /// from a cancellation of a call that was sent.
    pub fn not_started(wall_ms: i64) -> Self {
        Self {
            notices: vec![Notice::StoppedBeforeStart],
            ..Self::cancelled(wall_ms)
        }
    }

    /// Whether this outcome says that nothing was sent ([`ExecutionOutcome::not_started`]).
    pub fn is_not_started(&self) -> bool {
        self.termination == Termination::Cancelled
            && self.failure.is_none()
            && self.notices.contains(&Notice::StoppedBeforeStart)
    }

    /// An outcome that was cancelled.
    pub fn cancelled(wall_ms: i64) -> Self {
        Self {
            termination: Termination::Cancelled,
            failure: None,
            payload: Value::Null,
            full_payload: None,
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
    /// The refresh observes `interruption` and returns without a verdict
    /// for a server it could not reach: its tools stay pinned.
    fn refresh_pins(
        &self,
        pinned: &std::collections::BTreeMap<String, crate::tool::ImportRecord>,
        interruption: &Cancellation,
    ) -> Vec<String> {
        let _ = (pinned, interruption);
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

/// The text of a refusal that names unlisted entries: the first few, then how many more.
fn unlisted_message(paths: &[String]) -> String {
    const SHOWN: usize = 10;
    let shown: Vec<&str> = paths.iter().take(SHOWN).map(String::as_str).collect();
    let more = paths.len().saturating_sub(SHOWN);
    let mut text = format!(
        "the project holds {} entr{} the plan does not list, in a directory it would remove: {}",
        paths.len(),
        if paths.len() == 1 { "y" } else { "ies" },
        shown.join(", ")
    );
    if more > 0 {
        text.push_str(&format!(" and {more} more"));
    }
    text
}

/// How many paths of a deletion that left files the text names; the rest are counted. The JSON
/// report of the command carries every path.
const NOT_REMOVED_SHOWN: usize = 10;

/// The text of a deletion that left files behind: the task is deleted, the first paths
/// ([`NOT_REMOVED_SHOWN`]) are named and the rest counted, and the next start removes them
/// (RFC 006 §6). `task delete` and `task prune` word it the same way.
pub fn not_removed_text(task_id: &str, paths: &[String]) -> String {
    let shown: Vec<&str> = paths
        .iter()
        .take(NOT_REMOVED_SHOWN)
        .map(String::as_str)
        .collect();
    let more = paths.len().saturating_sub(NOT_REMOVED_SHOWN);
    let mut text = format!(
        "task {task_id} is deleted; {} path(s) could not be removed and are left to the next start: {}",
        paths.len(),
        shown.join(", ")
    );
    if more > 0 {
        text.push_str(&format!(" and {more} more"));
    }
    text
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
    /// The task's directories could not be given the permissions its tools
    /// need (Windows: the entry of the task's capability on its `tree` and
    /// `tmp`, handoff 011o). The workspace was not made. `reason` is the
    /// refusal's own text, beginning `acl_not_applied:`, as every other
    /// refusal of a permission write has; the error kind is `unavailable`
    /// with the component `permissions` (external design §18).
    #[error("{reason}")]
    PermissionsNotApplied {
        /// The refusal, `acl_not_applied: <cause>`.
        reason: String,
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
    /// A directory the promotion would remove holds project entries the plan does not list
    /// (REQ-WS-007): the top-most unlisted entries are named, so that the user decides.
    #[error("promotion refused: {}", unlisted_message(paths))]
    Unlisted {
        /// The unlisted entries, top-most first, sorted.
        paths: Vec<String>,
    },
    /// A deletion left files it could not remove (RFC 006 §6): every removal was attempted, and
    /// each failure is named. The task is deleted (its rows are gone); the next start tries
    /// these paths again.
    #[error("{}", not_removed_text(&task_id.to_string(), paths))]
    NotRemoved {
        /// The deleted task.
        task_id: TaskId,
        /// The paths, relative to the data directory.
        paths: Vec<String>,
    },
    /// A reversal would overwrite paths edited since the promotion (REQ-WS-007).
    #[error("reversal refused: {} path(s) were edited since the promotion: {}", paths.len(), paths.join(", "))]
    EditedSince {
        /// The edited paths.
        paths: Vec<String>,
    },
    /// The operation is not valid now (the plan is stale, the task not terminal).
    #[error("refused: {reason}")]
    Refused {
        /// Why.
        reason: String,
    },
    /// Part of the task's tree could not be read, so it could not be walked (a snapshot, a
    /// rollback, the quota check): a directory or a file whose permissions forbid it. Nothing
    /// was changed; the user restores the permissions of the path and tries again.
    #[error(
        "part of the workspace could not be read: {path}; restore its permissions, then try again"
    )]
    Unreadable {
        /// The path, relative to the task's tree; cleaned of control characters and bounded.
        path: String,
    },
    /// As `Unreadable`, for a read that failed for a reason other than permissions (an entry
    /// that vanished, a failing device): the error names the path and the kind of the failure,
    /// and gives no advice it cannot back. The task halts the same way.
    #[error("part of the workspace could not be read: {path} ({kind})")]
    ReadFailed {
        /// The path, relative to the task's tree; cleaned of control characters and bounded.
        path: String,
        /// The kind of the I/O error, in words (`entity not found`).
        kind: String,
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

/// The reading half of the workspace interface (external design §14.6;
/// REQ-PERS-013, RFC 013): what a read command needs to show a task's
/// footprint and a promotion's plan. It changes nothing on disk. A read
/// command holds this type, never [`WorkspaceProvider`].
pub trait WorkspaceRead: Send + Sync {
    /// A snapshot's manifest, verified against the recorded hash.
    fn manifest(&self, snapshot: &SnapshotRecord) -> Result<Manifest, WorkspaceError>;

    /// Phase 1 of a promotion: the diff against the base, conflicts refused (REQ-WS-007).
    fn promotion_plan(
        &self,
        task_id: TaskId,
        base: &SnapshotRecord,
        project_root: &str,
    ) -> Result<PromotionPlan, WorkspaceError>;

    /// Bytes of the working tree.
    fn tree_bytes(&self, task_id: TaskId) -> Result<u64, WorkspaceError>;
}

/// Task workspaces: materialization, tools over the tree, quota (external design §14; REQ-WS-002, REQ-WS-008).
pub trait WorkspaceProvider: WorkspaceRead {
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

    /// Materializes a new task's tree from a snapshot and prepares its `fork` snapshot (REQ-TASK-004).
    fn materialize_from(
        &self,
        task_id: TaskId,
        snapshot_id: SnapshotId,
        from: &SnapshotRecord,
        scope: &ScopeSpec,
        now: crate::clock::Instant,
    ) -> Result<(WorkspaceInfo, PreparedSnapshot), WorkspaceError>;

    /// Phase 2, first step: backs the targets up (a prepared `backup` snapshot) and returns the
    /// promotion, `applying`, with the entries it will write. Nothing in the project is touched:
    /// a stale plan, or a project entry that is not its entry in the `base` snapshot, is refused.
    /// The runtime commits the backup and records the promotion before the next step.
    fn promote_backup(
        &self,
        task_id: TaskId,
        request: &PromotionRequest<'_>,
    ) -> Result<(PreparedSnapshot, Promotion), WorkspaceError>;

    /// Phase 2, second step: applies a promotion that is recorded `applying`. Each entry is
    /// compared with its entry in the `base` snapshot before it is written; nothing is removed
    /// that the promotion does not list. An error stops it there.
    fn promote_apply(
        &self,
        task_id: TaskId,
        promotion: &Promotion,
        base: &SnapshotRecord,
    ) -> Result<(), WorkspaceError>;

    /// What reversing a promotion would do: the paths it restores or removes, and the ones edited
    /// since. Nothing is changed.
    fn promotion_revert_plan(
        &self,
        promotion: &Promotion,
        backup: &SnapshotRecord,
    ) -> Result<RevertPlan, WorkspaceError>;

    /// Reverses a promotion from its backup, also the part of one that was applied: a path is
    /// restored while it holds what the promotion wrote or is missing. A path edited since is kept
    /// and reported in `left`, or, with [`EditedPaths::Overwrite`], restored too.
    fn revert_promotion(
        &self,
        promotion: &Promotion,
        backup: &SnapshotRecord,
        edited: EditedPaths,
    ) -> Result<RestoreReport, WorkspaceError>;

    /// Removes the files the database no longer references (RFC 006 §6).
    fn delete_files(&self, task_id: TaskId, rows: &DeletedRows) -> Result<(), WorkspaceError>;
}

/// What a reversal does with a path edited since the promotion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditedPaths {
    /// Keeps it as it is and reports it.
    Keep,
    /// Restores it too: the user confirmed.
    Overwrite,
}

/// What phase 2 of a promotion is given (REQ-WS-007).
#[derive(Clone, Copy, Debug)]
pub struct PromotionRequest<'a> {
    /// The plan the user confirmed.
    pub plan: &'a PromotionPlan,
    /// The task's base snapshot, which the project's entries are compared with.
    pub base: &'a SnapshotRecord,
    /// The project root.
    pub project_root: &'a str,
    /// The identifier of the backup snapshot.
    pub backup_id: SnapshotId,
    /// The identifier of the promotion.
    pub promotion_id: PromotionId,
    /// When.
    pub now: crate::clock::Instant,
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
    /// Denials by reason (REQ-OBS-008); the events of 0.10 on carry it.
    #[serde(default)]
    pub denials_by_reason: std::collections::BTreeMap<String, u64>,
    /// Denials by the grant kind that would allow them (REQ-OBS-008).
    #[serde(default)]
    pub denials_by_grant_kind: std::collections::BTreeMap<String, u64>,
    /// Halts by reason.
    pub halts_by_reason: std::collections::BTreeMap<String, u64>,
    /// Approvals by decision.
    pub approvals_by_decision: std::collections::BTreeMap<String, u64>,
    /// The decisions about a plan-first task's plan, by decision (from 0.13; REQ-TASK-015):
    /// `approved`, `rejected`, `expired`. Apart from the actions' approvals, so that the ratio
    /// of those is not changed by the plans. Absent from the JSON when there is none.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub plan_approvals_by_decision: std::collections::BTreeMap<String, u64>,
    /// Finished actions by status.
    pub actions_by_status: std::collections::BTreeMap<String, u64>,
    /// The context and model measures over measured steps.
    #[serde(default)]
    pub context: ContextMetrics,
    /// Milliseconds the executions waited for a slot among the user's runtimes (REQ-OBS-009): the
    /// waits that ended in an admission, summed over the task, or over every task.
    #[serde(default)]
    pub admission_wait_ms: u64,
    /// How many such waits there were.
    #[serde(default)]
    pub admission_waits: u64,
    /// The names above that this release does not know, which a newer release wrote
    /// (REQ-MAIN-006): each with the group that holds it (`events_by_name`, `tasks_by_state`,
    /// `halts_by_reason`, …). They are counted in their group like any other name. Empty, and
    /// absent from the JSON, when every name is known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_to_this_release: Vec<crate::newer::UnknownWord>,
    /// Milliseconds the model requests waited for the engine's gate (REQ-OBS-009), summed.
    #[serde(default)]
    pub engine_wait_ms: u64,
    /// How many requests waited for it.
    #[serde(default)]
    pub engine_waits: u64,
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
            _interruption: &Cancellation,
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
        remote: std::sync::atomic::AtomicBool,
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
                remote: std::sync::atomic::AtomicBool::new(false),
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
        pub fn remote(self) -> Self {
            self.set_remote(true);
            self
        }

        /// From now on the backend declares itself `remote` (or `local`): the configuration was
        /// edited between two starts of a task (REQ-SEC-010 tests).
        pub fn set_remote(&self, remote: bool) {
            self.remote
                .store(remote, std::sync::atomic::Ordering::SeqCst);
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
            if self.remote.load(std::sync::atomic::Ordering::SeqCst) {
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
                locality: if self.remote.load(std::sync::atomic::Ordering::SeqCst) {
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
        fail_restores: Mutex<bool>,
        unreadable: Mutex<Option<(String, bool, Option<String>)>>,
        tree: Mutex<Option<String>>,
        apply_hook: Mutex<Option<ApplyHook>>,
        reverted: Mutex<Vec<(PromotionId, EditedPaths)>>,
        fail_reverts: Mutex<bool>,
        edited: Mutex<Vec<String>>,
        /// The workspaces made and not removed: what a refused creation must not leave.
        materialized: Mutex<std::collections::BTreeSet<TaskId>>,
        /// From now on a materialization makes the workspace and then fails (a full disk).
        fail_materialize: Mutex<bool>,
        /// From now on `tools` fails (the provider cannot be opened).
        fail_tools: Mutex<bool>,
        /// For each removal, the holders of the tool provider at that moment.
        removed_with_holders: Mutex<Vec<usize>>,
    }

    /// What a test runs when a promotion is applied; an error fails the apply.
    pub type ApplyHook = Box<dyn Fn(&Promotion) -> Result<(), WorkspaceError> + Send + Sync>;

    impl FakeWorkspaces {
        /// Serves `tools` for every task.
        pub fn new(tools: Arc<dyn ToolProvider>) -> Self {
            Self {
                tools,
                remaining: Mutex::new(None),
                snapshots: Mutex::new(Vec::new()),
                restores: Mutex::new(Vec::new()),
                fail_restores: Mutex::new(false),
                unreadable: Mutex::new(None),
                tree: Mutex::new(None),
                apply_hook: Mutex::new(None),
                reverted: Mutex::new(Vec::new()),
                fail_reverts: Mutex::new(false),
                edited: Mutex::new(Vec::new()),
                materialized: Mutex::new(Default::default()),
                fail_materialize: Mutex::new(false),
                fail_tools: Mutex::new(false),
                removed_with_holders: Mutex::new(Vec::new()),
            }
        }

        /// Sets the content of the fake tree: a snapshot taken from now on has a
        /// manifest hash that depends on `content` alone (the default tree is empty).
        pub fn set_tree(&self, content: &str) {
            *self.tree.lock().expect("fake lock") = Some(content.to_owned());
        }

        /// From now on every restore fails with a storage error.
        pub fn fail_restores(&self) {
            *self.fail_restores.lock().expect("fake lock") = true;
        }

        /// Sets the remaining workspace bytes reported.
        pub fn set_remaining(&self, bytes: Option<u64>) {
            *self.remaining.lock().expect("fake lock") = bytes;
        }

        /// From now on the tree has a part at `path` that cannot be read: a snapshot, a
        /// restore and the quota check fail with `Unreadable`, until `make_readable`.
        pub fn make_unreadable(&self, path: &str) {
            *self.unreadable.lock().expect("fake lock") = Some((path.to_owned(), true, None));
        }

        /// As `make_unreadable`, for a snapshot and a restore only: the quota check, which
        /// measures the tree, still reads it.
        pub fn make_unreadable_to_snapshots(&self, path: &str) {
            *self.unreadable.lock().expect("fake lock") = Some((path.to_owned(), false, None));
        }

        /// As `make_unreadable_to_snapshots`, for a part that fails to read for a reason other
        /// than its permissions (`kind` is the reason's name): `ReadFailed`, not `Unreadable`.
        pub fn make_unreadable_to_snapshots_for_another_reason(&self, path: &str, kind: &str) {
            *self.unreadable.lock().expect("fake lock") =
                Some((path.to_owned(), false, Some(kind.to_owned())));
        }

        /// The permissions are restored: the tree can be read again.
        pub fn make_readable(&self) {
            *self.unreadable.lock().expect("fake lock") = None;
        }

        fn readable(&self, measuring: bool) -> Result<(), WorkspaceError> {
            match self.unreadable.lock().expect("fake lock").clone() {
                Some((path, everything, kind)) if everything || !measuring => Err(match kind {
                    None => WorkspaceError::Unreadable { path },
                    Some(kind) => WorkspaceError::ReadFailed { path, kind },
                }),
                _ => Ok(()),
            }
        }
    }

    impl WorkspaceRead for FakeWorkspaces {
        fn manifest(&self, _snapshot: &SnapshotRecord) -> Result<Manifest, WorkspaceError> {
            Ok(Manifest::default())
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

        fn tree_bytes(&self, _task_id: TaskId) -> Result<u64, WorkspaceError> {
            Ok(0)
        }
    }

    impl WorkspaceProvider for FakeWorkspaces {
        fn materialize(
            &self,
            task_id: TaskId,
            scope: &ScopeSpec,
        ) -> Result<WorkspaceInfo, WorkspaceError> {
            self.materialized.lock().expect("fake lock").insert(task_id);
            if *self.fail_materialize.lock().expect("fake lock") {
                return Err(WorkspaceError::Storage {
                    detail: "injected".into(),
                });
            }
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
            if *self.fail_tools.lock().expect("fake lock") {
                return Err(WorkspaceError::Missing);
            }
            Ok(Arc::clone(&self.tools))
        }

        fn remaining_bytes(&self, _task_id: TaskId) -> Result<Option<u64>, WorkspaceError> {
            self.readable(true)?;
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
            self.readable(false)?;
            let manifest_hash = match self.tree.lock().expect("fake lock").as_deref() {
                Some(content) => crate::hash::hash_bytes(content.as_bytes()),
                None => Manifest::default().hash(),
            };
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
            self.readable(false)?;
            if *self.fail_restores.lock().expect("fake lock") {
                return Err(WorkspaceError::Storage {
                    detail: "restore".into(),
                });
            }
            self.restores
                .lock()
                .expect("fake lock")
                .push(snapshot.snapshot_id);
            Ok(RestoreReport::default())
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

        fn promote_backup(
            &self,
            task_id: TaskId,
            request: &PromotionRequest<'_>,
        ) -> Result<(PreparedSnapshot, Promotion), WorkspaceError> {
            let record = SnapshotRecord {
                snapshot_id: request.backup_id,
                task_id,
                reason: SnapshotReason::Backup,
                manifest_hash: Manifest::default().hash(),
                bytes: 0,
                files: 0,
                created_at: request.now,
                action_id: None,
                project_root: Some(request.project_root.to_owned()),
            };
            let promotion = Promotion {
                promotion_id: request.promotion_id,
                task_id,
                backup_snapshot: request.backup_id,
                project_root: request.project_root.to_owned(),
                files: request.plan.changes.clone(),
                applied_at: request.now,
                reversed_at: None,
                state: crate::snapshot::PromotionState::Applying,
                after: vec![],
                reason: None,
                restored: true,
            };
            Ok((
                PreparedSnapshot {
                    record,
                    objects: vec![],
                    reused: false,
                },
                promotion,
            ))
        }

        fn promote_apply(
            &self,
            _task_id: TaskId,
            promotion: &Promotion,
            _base: &SnapshotRecord,
        ) -> Result<(), WorkspaceError> {
            match self.apply_hook.lock().expect("fake lock").as_ref() {
                Some(hook) => hook(promotion),
                None => Ok(()),
            }
        }

        fn promotion_revert_plan(
            &self,
            promotion: &Promotion,
            _backup: &SnapshotRecord,
        ) -> Result<RevertPlan, WorkspaceError> {
            Ok(RevertPlan {
                promotion_id: promotion.promotion_id,
                entries: vec![],
                conflicts: self.edited.lock().expect("fake lock").clone(),
                edits_detected: true,
            })
        }

        fn revert_promotion(
            &self,
            promotion: &Promotion,
            _backup: &SnapshotRecord,
            edited: EditedPaths,
        ) -> Result<RestoreReport, WorkspaceError> {
            if *self.fail_reverts.lock().expect("fake lock") {
                return Err(WorkspaceError::Storage {
                    detail: "injected".into(),
                });
            }
            self.reverted
                .lock()
                .expect("fake lock")
                .push((promotion.promotion_id, edited));
            Ok(RestoreReport::default())
        }

        fn delete_files(&self, task_id: TaskId, _rows: &DeletedRows) -> Result<(), WorkspaceError> {
            self.materialized
                .lock()
                .expect("fake lock")
                .remove(&task_id);
            self.removed_with_holders
                .lock()
                .expect("fake lock")
                .push(Arc::strong_count(&self.tools));
            self.snapshots
                .lock()
                .expect("fake lock")
                .retain(|s| s.task_id != task_id);
            Ok(())
        }
    }

    impl FakeWorkspaces {
        /// The tasks whose workspace was made and not removed, in order.
        pub fn materialized(&self) -> Vec<TaskId> {
            self.materialized
                .lock()
                .expect("fake lock")
                .iter()
                .copied()
                .collect()
        }

        /// From now on a materialization makes its workspace and then fails.
        pub fn fail_materialize(&self) {
            *self.fail_materialize.lock().expect("fake lock") = true;
        }

        /// From now on the tool provider of a task cannot be opened.
        pub fn fail_tools(&self) {
            *self.fail_tools.lock().expect("fake lock") = true;
        }

        /// For each removal of a workspace, the number of holders of the tool provider
        /// (the fake's own, and whoever else holds a handle) at that moment.
        pub fn removed_with_holders(&self) -> Vec<usize> {
            self.removed_with_holders.lock().expect("fake lock").clone()
        }

        /// Snapshots taken, in order.
        pub fn snapshots(&self) -> Vec<SnapshotRecord> {
            self.snapshots.lock().expect("fake lock").clone()
        }

        /// Restores performed, in order.
        pub fn restores(&self) -> Vec<SnapshotId> {
            self.restores.lock().expect("fake lock").clone()
        }

        /// From now on a promotion is applied by running `hook`, whose error fails the apply.
        pub fn on_promote_apply(&self, hook: ApplyHook) {
            *self.apply_hook.lock().expect("fake lock") = Some(hook);
        }

        /// From now on every reversal of a promotion fails with a storage error.
        pub fn fail_reverts(&self) {
            *self.fail_reverts.lock().expect("fake lock") = true;
        }

        /// From now on reversals of a promotion work again.
        pub fn allow_reverts(&self) {
            *self.fail_reverts.lock().expect("fake lock") = false;
        }

        /// Promotions reverted from their backups, in order, with how edited paths were treated.
        pub fn reverted(&self) -> Vec<(PromotionId, EditedPaths)> {
            self.reverted.lock().expect("fake lock").clone()
        }

        /// From now on the plan of a reversal reports these paths as edited since.
        pub fn set_edited(&self, paths: &[&str]) {
            *self.edited.lock().expect("fake lock") =
                paths.iter().map(|p| (*p).to_owned()).collect();
        }
    }

    /// An in-memory persistence with the same write-ordering rules as the
    /// SQLite store, for deterministic runtime tests (REQ-ARCH-006).
    #[derive(Default)]
    pub struct FakePersistence {
        inner: Mutex<FakeInner>,
        /// Run by the write method of that name before it writes, outside the
        /// fake's own lock (a test's stop signal that falls at the start of an action).
        before_write: Mutex<std::collections::HashMap<&'static str, WriteHook>>,
        /// The write methods that fail with a storage error (a full disk).
        failing: Mutex<std::collections::BTreeSet<&'static str>>,
        /// Records of a newer release that the store holds as text: they are read as text and
        /// are no typed record of the fake (REQ-MAIN-006).
        newer: Mutex<Vec<(TaskId, crate::newer::RawRecord)>>,
    }

    /// A test's hook run before a write method of the fake writes.
    type WriteHook = Arc<dyn Fn() + Send + Sync>;

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
        /// From now on the write method `name` (`"authorized"`, `"running"`, ...)
        /// runs `hook` first, before it writes: the place where a test raises a
        /// stop signal just before an action's check (`"authorized"`) or inside the
        /// durable record of its start (`"running"`). The hook runs outside the
        /// fake's own lock, so it may read the store, from this thread or from
        /// another.
        pub fn before_write(&self, name: &'static str, hook: Arc<dyn Fn() + Send + Sync>) {
            self.before_write
                .lock()
                .expect("fake lock")
                .insert(name, hook);
        }

        /// From now on the write method `name` fails with a storage error, and writes nothing.
        pub fn fail_write(&self, name: &'static str) {
            self.failing.lock().expect("fake lock").insert(name);
        }

        /// The store holds `text` as a record of `kind` of the task, as it would a record that a
        /// newer release wrote: it is returned by `raw_records` and by nothing typed.
        pub fn hold_raw(&self, task_id: TaskId, kind: crate::newer::RecordKind, text: &str) {
            self.newer.lock().expect("fake lock").push((
                task_id,
                crate::newer::RawRecord {
                    kind,
                    key: "held".into(),
                    text: text.to_owned(),
                },
            ));
        }

        /// The write methods called so far, in order.
        pub fn writes(&self) -> Vec<&'static str> {
            self.inner.lock().expect("fake lock").writes.clone()
        }

        /// Drops the stored rejection details of the task's steps, as a store written before
        /// schema version 3 holds them: the code stays, the detail is gone.
        pub fn forget_rejection_details(&self, task_id: TaskId) {
            for step in &mut self.inner.lock().expect("fake lock").steps {
                if step.task_id == task_id {
                    step.rejection_detail = None;
                }
            }
        }

        /// Every blob so far, in the order they were stored.
        pub fn blobs(&self) -> Vec<(BlobId, Vec<u8>)> {
            self.inner.lock().expect("fake lock").blobs.clone()
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
            let hook = self
                .before_write
                .lock()
                .expect("fake lock")
                .get(name)
                .cloned();
            if let Some(hook) = hook {
                hook();
            }
            if self.failing.lock().expect("fake lock").contains(name) {
                return Err(PersistenceError::Storage {
                    detail: "injected".into(),
                });
            }
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

    impl PersistenceRead for FakePersistence {
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
                    unknown: Vec::new(),
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

        fn load_plans(&self, task_id: TaskId) -> Result<Vec<PlanVersion>, PersistenceError> {
            let mut plans: Vec<PlanVersion> = self
                .inner
                .lock()
                .expect("fake lock")
                .plans
                .iter()
                .filter(|p| p.task_id == task_id)
                .cloned()
                .collect();
            plans.sort_by_key(|p| p.version);
            Ok(plans)
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

        fn read_events_since(
            &self,
            position: u64,
            task_id: Option<TaskId>,
            limit: usize,
        ) -> Result<Vec<PositionedRead>, PersistenceError> {
            Ok(self
                .events_since(position, task_id, limit)?
                .into_iter()
                .map(|e| PositionedRead {
                    position: e.position,
                    record: crate::newer::Read::Known(e.record),
                })
                .collect())
        }

        fn raw_records(
            &self,
            kind: crate::newer::RecordKind,
            task_id: TaskId,
        ) -> Result<Vec<crate::newer::RawRecord>, PersistenceError> {
            use crate::newer::RecordKind as K;
            fn raw<T: Serialize>(
                kind: K,
                key: impl ToString,
                value: &T,
            ) -> crate::newer::RawRecord {
                crate::newer::RawRecord {
                    kind,
                    key: key.to_string(),
                    text: serde_json::to_string(value).expect("a record serializes"),
                }
            }
            let held: Vec<crate::newer::RawRecord> = self
                .newer
                .lock()
                .expect("fake lock")
                .iter()
                .filter(|(t, r)| *t == task_id && r.kind == kind)
                .map(|(_, r)| r.clone())
                .collect();
            let inner = self.inner.lock().expect("fake lock");
            let mut records: Vec<crate::newer::RawRecord> = match kind {
                K::Task => inner
                    .tasks
                    .iter()
                    .filter(|t| t.task_id == task_id)
                    .map(|t| raw(kind, t.task_id, t))
                    .collect(),
                K::Step => inner
                    .steps
                    .iter()
                    .filter(|s| s.task_id == task_id)
                    .map(|s| raw(kind, s.step_id, s))
                    .collect(),
                K::Action => inner
                    .actions
                    .iter()
                    .filter(|a| a.task_id == task_id)
                    .map(|a| raw(kind, a.action_id, a))
                    .collect(),
                K::Execution => Vec::new(),
                K::Observation => Vec::new(),
                K::Approval => inner
                    .approvals
                    .iter()
                    .filter(|a| a.task_id == task_id)
                    .map(|a| raw(kind, a.approval_id, a))
                    .collect(),
                K::Question => inner
                    .questions
                    .iter()
                    .filter(|q| q.task_id == task_id)
                    .map(|q| raw(kind, q.question_id, q))
                    .collect(),
                K::Grant => inner
                    .grants
                    .iter()
                    .filter(|g| g.task_id == task_id)
                    .map(|g| raw(kind, g.grant_id, g))
                    .collect(),
                K::Plan => inner
                    .plans
                    .iter()
                    .filter(|p| p.task_id == task_id)
                    .map(|p| raw(kind, p.version, p))
                    .collect(),
                K::Event => inner
                    .events
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.task_id == Some(task_id))
                    .map(|(i, e)| raw(kind, i + 1, e))
                    .collect(),
                K::Snapshot => inner
                    .snapshots
                    .iter()
                    .filter(|s| s.task_id == task_id)
                    .map(|s| raw(kind, s.snapshot_id, s))
                    .collect(),
                K::Promotion => Vec::new(),
            };
            records.extend(held);
            Ok(records)
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

        fn promotions_applying(&self) -> Result<Vec<Promotion>, PersistenceError> {
            Ok(self
                .inner
                .lock()
                .expect("fake lock")
                .promotions
                .iter()
                .filter(|p| p.state == crate::snapshot::PromotionState::Applying)
                .cloned()
                .collect())
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

        fn record_not_started(
            &self,
            action: &Action,
            observation: &Observation,
            event: &EventRecord,
        ) -> Result<(), PersistenceError> {
            self.write("not_started", |i| {
                expect_status(i, action, &["authorized", "running"], "not_started")?;
                upsert(&mut i.actions, action, |a| a.action_id == action.action_id);
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

        fn record_plan_step(
            &self,
            step: &Step,
            task: &Task,
            plan: Option<&PlanVersion>,
            events: &[EventRecord],
        ) -> Result<(), PersistenceError> {
            self.write("plan_step", |i| {
                if let Some(plan) = plan {
                    i.plans.push(plan.clone());
                }
                upsert(&mut i.steps, step, |s| s.step_id == step.step_id);
                upsert(&mut i.tasks, task, |t| t.task_id == task.task_id);
                i.events.extend(events.iter().cloned());
                Ok(())
            })
        }

        fn append_event(&self, event: &EventRecord) -> Result<(), PersistenceError> {
            self.write("event", |i| {
                i.events.push(event.clone());
                Ok(())
            })
        }

        fn put_blob(
            &self,
            _task_id: TaskId,
            bytes: &[u8],
            max_bytes: usize,
        ) -> Result<BlobId, PersistenceError> {
            if bytes.len() > max_bytes {
                return Err(PersistenceError::Storage {
                    detail: "blob_too_large".into(),
                });
            }
            self.write("blob", |i| {
                let id = BlobId::from_uuid(uuid::Uuid::from_u128(i.blobs.len() as u128 + 1));
                i.blobs.push((id, bytes.to_vec()));
                Ok(id)
            })
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
    /// The database schema is older than this runtime's and a read-only open
    /// cannot migrate it (REQ-PERS-013): the next command that writes does.
    #[error(
        "migration pending: the database schema is version {found}, this runtime's is {supported}; the next command that writes migrates it"
    )]
    MigrationPending {
        /// Found in the database (0 when it has no schema yet).
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
    /// A record holds a word that a newer release of mimlys wrote and this release does not know
    /// (REQ-MAIN-006; external design §22). A command that only reads shows the record as it was
    /// written, marked unknown to this release; a command that would change or run its task
    /// refuses with this, so that no word is ever taken for another.
    #[error(
        "{record} holds {field} `{word}`, which is unknown to this release: the release of mimlys that wrote it is needed"
    )]
    NewerRecord {
        /// The kind of record that holds it (`task`, `event`, `action`, …).
        record: String,
        /// The field that holds the word.
        field: String,
        /// The word as written.
        word: String,
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
    /// The words of the task's own record that this release does not know, when it holds any: a
    /// state a newer release wrote, a wait kind, a reason (REQ-MAIN-006). Empty, and absent from
    /// the JSON, for a task this release reads whole.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<crate::newer::UnknownWord>,
}

/// An event with its position in the log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionedEvent {
    /// Monotonic position.
    pub position: u64,
    /// The event.
    pub record: EventRecord,
}

/// An event with its position, as a reader meets it: whole, or holding a word that a newer
/// release wrote (REQ-MAIN-006). It serializes as [`PositionedEvent`] does, the record as written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PositionedRead {
    /// Monotonic position.
    pub position: u64,
    /// The event, or the record as written with the words this release does not know.
    pub record: crate::newer::Read<EventRecord>,
}

/// The reading half of the store (external design §16; REQ-PERS-013, RFC
/// 013): every method only reads. The read-only handle of the persistence
/// crate implements this trait and nothing else, so that a command that
/// holds it cannot call a writing method: the type has none.
pub trait PersistenceRead: Send + Sync {
    /// Loads a task.
    fn load_task(&self, task_id: TaskId) -> Result<Option<Task>, PersistenceError>;

    /// Lists tasks, newest first.
    fn list_tasks(&self) -> Result<Vec<TaskSummary>, PersistenceError>;

    /// Tasks whose state has this name.
    fn tasks_in_state(&self, state: &str) -> Result<Vec<TaskId>, PersistenceError>;

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

    /// Every plan version of the task, oldest first.
    fn load_plans(&self, task_id: TaskId) -> Result<Vec<PlanVersion>, PersistenceError>;

    /// The task's snapshots, oldest first.
    fn load_snapshots(&self, task_id: TaskId) -> Result<Vec<SnapshotRecord>, PersistenceError>;

    /// One snapshot.
    fn load_snapshot(
        &self,
        snapshot_id: SnapshotId,
    ) -> Result<Option<SnapshotRecord>, PersistenceError>;

    /// The workspace's recorded state, if any.
    fn load_workspace_state(
        &self,
        task_id: TaskId,
    ) -> Result<Option<WorkspaceState>, PersistenceError>;

    /// The task's promotions, oldest first.
    fn load_promotions(&self, task_id: TaskId) -> Result<Vec<Promotion>, PersistenceError>;

    /// The promotions of every task that are recorded `applying`: found at start, they did not
    /// finish (external design §16.3).
    fn promotions_applying(&self) -> Result<Vec<Promotion>, PersistenceError>;

    /// How many manifests reference each of these objects, and their sizes.
    fn object_references(
        &self,
        hashes: &[crate::hash::Hash],
    ) -> Result<Vec<(crate::hash::Hash, u64, u64)>, PersistenceError>;

    /// Bytes of the task's blobs.
    fn blob_bytes(&self, task_id: TaskId) -> Result<u64, PersistenceError>;

    /// Metrics over all tasks, or one task.
    fn metrics(&self, task_id: Option<TaskId>) -> Result<Metrics, PersistenceError>;

    /// Events after `position`, oldest first, at most `limit`.
    fn events_since(
        &self,
        position: u64,
        task_id: Option<TaskId>,
        limit: usize,
    ) -> Result<Vec<PositionedEvent>, PersistenceError>;

    /// Events after `position`, oldest first, at most `limit`, as a reader meets them: an event
    /// that holds a word this release does not know is kept as written, marked, and never fails
    /// the read (REQ-MAIN-006). [`PersistenceRead::events_since`] refuses it instead.
    fn read_events_since(
        &self,
        position: u64,
        task_id: Option<TaskId>,
        limit: usize,
    ) -> Result<Vec<PositionedRead>, PersistenceError>;

    /// The records of one kind that the task owns, as stored and unread, in the order the typed
    /// loads give them: the one place where a record is not yet a type, for the readers and the
    /// check of a task's records (`newer`).
    fn raw_records(
        &self,
        kind: crate::newer::RecordKind,
        task_id: TaskId,
    ) -> Result<Vec<crate::newer::RawRecord>, PersistenceError>;

    /// Reads a blob.
    fn get_blob(&self, blob_id: BlobId) -> Result<Vec<u8>, PersistenceError>;
}

/// Durable storage of tasks and everything they own (external design §16;
/// REQ-PERS-001 … 012). Every writing method is one transaction that also
/// appends the event it is given, and returns only after the commit
/// (REQ-PERS-003). The methods `record_proposed` … `record_step_outcome` are
/// the eight ordered lines of external design §16.2 (REQ-PERS-004). The
/// reading methods are [`PersistenceRead`]'s.
pub trait Persistence: PersistenceRead {
    /// Creates a task with its initial grants (line 0).
    fn create_task(
        &self,
        task: &Task,
        grants: &[Grant],
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;

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

    /// An authorized action closed `cancelled` without starting, because the
    /// user's stop signal came before it began (REQ-CLI-005): the terminal
    /// status, the observation that says so and the event, in one write. There
    /// is no execution: nothing ran. The action is `authorized`, or `running`
    /// when it was recorded as begun and then stopped at a wait made before
    /// anything was sent (handoff 010f: the MCP pin check): its `action.started`
    /// stands, and it has no execution record either.
    fn record_not_started(
        &self,
        action: &Action,
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

    /// A step of a plan-first task's plan wait (REQ-TASK-015), in one write: the step, the task
    /// as it is now, the plan version when this write makes it, and the events, in order. The
    /// plan is put to the user (the step open in the `wait` phase, the task waiting), or the
    /// user's decision, the expiry or a cancellation ends the wait (the step finished, the task
    /// running, halted or cancelled).
    fn record_plan_step(
        &self,
        step: &Step,
        task: &Task,
        plan: Option<&PlanVersion>,
        events: &[EventRecord],
    ) -> Result<(), PersistenceError>;

    /// Records a snapshot whose files are durable, and the objects it references
    /// (counts incremented in the same transaction) (REQ-WS-004, REQ-PERS-009).
    fn record_snapshot(
        &self,
        snapshot: &SnapshotRecord,
        objects: &[(crate::hash::Hash, u64)],
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;

    /// The workspace's lifecycle state (REQ-WS-009).
    fn record_workspace_state(
        &self,
        task_id: TaskId,
        state: WorkspaceState,
        event: Option<&EventRecord>,
    ) -> Result<(), PersistenceError>;

    /// A promotion applied (REQ-WS-007).
    fn record_promotion(
        &self,
        promotion: &Promotion,
        event: &EventRecord,
    ) -> Result<(), PersistenceError>;

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

    /// Stores a blob of at most `max_bytes` and returns its id (file first, then the row).
    /// The caller names the bound that applies to what it stores (LIM-S4 for a script's
    /// stream, LIM-H4 for a host tool's); more is refused, never cut.
    fn put_blob(
        &self,
        task_id: TaskId,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<BlobId, PersistenceError>;
}
