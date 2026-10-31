//! The built-in file tools (external design §11; REQ-TOOL-003, REQ-TOOL-004,
//! REQ-TOOL-006 … 008). Every operation goes through one `cap_std::fs::Dir`
//! handle rooted at the workspace: there is no path-string open, and a
//! symbolic link that leaves the root fails at open time.

mod delete_path;
mod list_dir;
mod read_file;
mod replace_in_file;
mod search_files;
mod write_file;

use cap_std::fs::Dir;
use mimlys_core::action::{EffectKind, EffectView, ExecutionMetrics};
use mimlys_core::limits::LIM_O4_BUILTIN_DEADLINE_MS;
use mimlys_core::path::WorkspacePath;
use mimlys_core::spi::{BoxFuture, Cancellation, ExecutionContext, ExecutionOutcome, ToolProvider};
use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};
use mimlys_core::validate::AuthorizedAction;
use serde_json::{Map, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The built-in tools over one workspace.
#[derive(Clone)]
pub struct BuiltinTools {
    dir: Arc<Dir>,
}

impl std::fmt::Debug for BuiltinTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BuiltinTools")
    }
}

/// A tool-level failure, mapped to `tool_error` with a fixed detail.
#[derive(Debug)]
pub(crate) struct ToolError {
    pub(crate) detail: &'static str,
    pub(crate) payload: Value,
}

impl ToolError {
    pub(crate) fn new(detail: &'static str) -> Self {
        Self {
            detail,
            payload: Value::Null,
        }
    }

    pub(crate) fn io(detail: &'static str, err: &std::io::Error) -> Self {
        Self {
            detail,
            payload: serde_json::json!({ "kind": format!("{:?}", err.kind()) }),
        }
    }
}

pub(crate) type ToolResult = Result<(Value, Vec<mimlys_core::observation::Notice>), ToolError>;

/// The outcome of a blocking tool body.
pub(crate) struct Body {
    pub(crate) dir: Arc<Dir>,
    pub(crate) args: Map<String, Value>,
    pub(crate) cancel: Cancellation,
}

impl BuiltinTools {
    /// Opens the workspace root as a capability handle (REQ-TOOL-004).
    pub fn open(root: &Path) -> std::io::Result<Self> {
        let dir = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
        Ok(Self { dir: Arc::new(dir) })
    }

    /// Wraps an existing handle.
    pub fn from_dir(dir: Dir) -> Self {
        Self { dir: Arc::new(dir) }
    }

    /// The six M1 definitions (external design §11).
    pub fn all_definitions() -> Vec<ToolDefinition> {
        vec![
            read_file::definition(),
            write_file::definition(),
            replace_in_file::definition(),
            list_dir::definition(),
            search_files::definition(),
            delete_path::definition(),
        ]
    }
}

pub(crate) fn path_param(name: &str, description: &str) -> Param {
    Param {
        name: name.into(),
        ty: ParamType::WorkspacePath,
        optional: false,
        description: description.into(),
    }
}

pub(crate) fn make_definition(
    name: &str,
    description: &str,
    params: Vec<Param>,
    kind: EffectKind,
) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input: ParamSchema { params },
        output: None,
        effect: EffectView {
            kind,
            reversible: true,
            idempotent: true,
        },
        deadline_ms: LIM_O4_BUILTIN_DEADLINE_MS,
        profile: ResourceProfile::Builtin,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

/// Reads a canonical workspace path argument the validator produced.
pub(crate) fn workspace_path(
    args: &Map<String, Value>,
    name: &str,
) -> Result<WorkspacePath, ToolError> {
    let text = args
        .get(name)
        .and_then(Value::as_str)
        .ok_or(ToolError::new("missing_path"))?;
    if text.is_empty() {
        return Ok(WorkspacePath::root());
    }
    WorkspacePath::canonicalize(text).map_err(|_| ToolError::new("invalid_path"))
}

pub(crate) fn int_arg(args: &Map<String, Value>, name: &str, default: i64) -> i64 {
    args.get(name).and_then(Value::as_i64).unwrap_or(default)
}

pub(crate) fn bool_arg(args: &Map<String, Value>, name: &str, default: bool) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(default)
}

pub(crate) fn str_arg<'a>(args: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    args.get(name).and_then(Value::as_str)
}

pub(crate) fn check_cancel(cancel: &Cancellation) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        Err(ToolError::new("cancelled"))
    } else {
        Ok(())
    }
}

impl ToolProvider for BuiltinTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        Self::all_definitions()
    }

    fn execute<'a>(
        &'a self,
        action: &'a AuthorizedAction,
        context: ExecutionContext,
    ) -> BoxFuture<'a, ExecutionOutcome> {
        let body = Body {
            dir: Arc::clone(&self.dir),
            args: action.arguments().clone(),
            cancel: context.cancel.clone(),
        };
        let tool = action.tool().to_owned();
        let deadline = Duration::from_millis(context.deadline_ms.max(0) as u64);
        Box::pin(async move {
            let started = Instant::now();
            let cancel = context.cancel.clone();
            let work = tokio::task::spawn_blocking(move || run(&tool, body));
            let result = tokio::time::timeout(deadline, work).await;
            let wall_ms = started.elapsed().as_millis() as i64;
            let metrics = ExecutionMetrics {
                wall_ms,
                ..ExecutionMetrics::default()
            };
            match result {
                Err(_elapsed) => ExecutionOutcome::timed_out(wall_ms),
                Ok(Err(_join)) => ExecutionOutcome {
                    termination: mimlys_core::action::Termination::HostFault {
                        detail: mimlys_core::action::HostFaultDetail::WorkerDied,
                    },
                    failure: Some(mimlys_core::action::FailureCause::SandboxFault {
                        detail: mimlys_core::action::HostFaultDetail::WorkerDied,
                    }),
                    payload: Value::Null,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    notices: Vec::new(),
                    metrics,
                },
                Ok(Ok(Ok((payload, notices)))) => ExecutionOutcome {
                    notices,
                    metrics,
                    ..ExecutionOutcome::succeeded(payload, wall_ms)
                },
                Ok(Ok(Err(err))) if err.detail == "cancelled" || cancel.is_cancelled() => {
                    ExecutionOutcome::cancelled(wall_ms)
                }
                Ok(Ok(Err(err))) => ExecutionOutcome::tool_error(err.detail, err.payload, wall_ms),
            }
        })
    }
}

fn run(tool: &str, body: Body) -> ToolResult {
    match tool {
        "read_file" => read_file::run(body),
        "write_file" => write_file::run(body),
        "replace_in_file" => replace_in_file::run(body),
        "list_dir" => list_dir::run(body),
        "search_files" => search_files::run(body),
        "delete_path" => delete_path::run(body),
        _ => Err(ToolError::new("unknown_builtin")),
    }
}

#[cfg(test)]
mod tests;
