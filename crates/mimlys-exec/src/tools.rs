//! `run_script` and `run_wasm` (external design §11; REQ-SBX-001): the two
//! tools whose executor is the worker. Their definitions take the configured
//! languages; `mode` derives the effect; both are non-idempotent.

use mimlys_core::action::EffectKind;
use mimlys_core::limits::{LIM_S2_DEADLINE_MS, LIM_W3_FILE_BYTES};
use mimlys_core::tool::{Param, ParamSchema, ParamType, Provider, ResourceProfile, ToolDefinition};

/// The `mode` values.
pub const MODES: [&str; 2] = ["read_only", "read_write"];

fn effect(kind: EffectKind) -> mimlys_core::action::EffectView {
    mimlys_core::action::EffectView {
        kind,
        reversible: true,
        idempotent: false,
    }
}

fn mode_param() -> Param {
    Param {
        name: "mode".into(),
        ty: ParamType::Enum {
            values: MODES.iter().map(|m| (*m).to_owned()).collect(),
        },
        optional: true,
        description: "read_only sees the workspace; read_write may change it (default).".into(),
    }
}

fn stdin_param() -> Param {
    Param {
        name: "stdin".into(),
        ty: ParamType::String {
            min_chars: 0,
            max_chars: LIM_W3_FILE_BYTES,
            max_bytes: Some(LIM_W3_FILE_BYTES),
        },
        optional: true,
        description: "Text given on standard input.".into(),
    }
}

fn args_param() -> Param {
    Param {
        name: "args".into(),
        ty: ParamType::StringArray {
            max_items: 64,
            max_chars: 4096,
        },
        optional: true,
        description: "Command-line arguments.".into(),
    }
}

/// `run_script` for the configured languages (the `language` enum).
pub fn run_script_definition(languages: &[String]) -> ToolDefinition {
    ToolDefinition {
        name: "run_script".into(),
        description: "Run a script in the sandbox with one of the listed languages; the result carries termination, exit_code, stdout and stderr.".into(),
        input: ParamSchema {
            params: vec![
                Param {
                    name: "language".into(),
                    ty: ParamType::Enum {
                        values: languages.to_vec(),
                    },
                    optional: false,
                    description: "The interpreter.".into(),
                },
                Param {
                    name: "source".into(),
                    ty: ParamType::String {
                        min_chars: 1,
                        max_chars: LIM_W3_FILE_BYTES,
                        max_bytes: Some(LIM_W3_FILE_BYTES),
                    },
                    optional: false,
                    description: "The program.".into(),
                },
                stdin_param(),
                args_param(),
                mode_param(),
            ],
        },
        output: None,
        // The write effect of the default mode; `read_only` is derived at validation (RFC 005 §5).
        effect: effect(EffectKind::WorkspaceWrite),
        deadline_ms: LIM_S2_DEADLINE_MS,
        profile: ResourceProfile::Sandbox,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

/// `run_wasm`.
pub fn run_wasm_definition() -> ToolDefinition {
    ToolDefinition {
        name: "run_wasm".into(),
        description: "Run a wasip1 module from the workspace in the sandbox; the result carries termination, exit_code, stdout and stderr.".into(),
        input: ParamSchema {
            params: vec![
                Param {
                    name: "module".into(),
                    ty: ParamType::WorkspacePath,
                    optional: false,
                    description: "The module file.".into(),
                },
                args_param(),
                stdin_param(),
                mode_param(),
            ],
        },
        output: None,
        effect: effect(EffectKind::WorkspaceWrite),
        deadline_ms: LIM_S2_DEADLINE_MS,
        profile: ResourceProfile::Sandbox,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

/// Whether the arguments ask for `read_only`.
pub fn is_read_only(args: &serde_json::Map<String, serde_json::Value>) -> bool {
    args.get("mode").and_then(|m| m.as_str()) == Some("read_only")
}

use crate::builtin::BuiltinTools;
use crate::worker::{Execution, Worker};
use mimlys_core::ids::TaskId;
use mimlys_core::spi::{BoxFuture, ExecutionContext, ExecutionOutcome, ToolProvider};
use mimlys_core::validate::AuthorizedAction;
use std::sync::Arc;

/// The task's tools: the built-in file tools in process, `run_script` and
/// `run_wasm` through the worker (REQ-ARCH-008).
pub struct WorkerTools {
    builtin: BuiltinTools,
    worker: Arc<Worker>,
    task_id: TaskId,
    workspace_root: String,
    languages: Vec<String>,
    deadline_ms: i64,
    /// The declared host tools (RFC 011 §6).
    host_tools: Vec<mimlys_core::host_tool::HostToolDeclaration>,
    /// Whether host tools are available on this machine (tier `full`, REQ-CONF-004).
    host_tools_available: bool,
    /// The worker for the dependency step (clause N lifted), when a tool declares `network` (RFC 011 §8).
    fetch_worker: Option<Arc<Worker>>,
}

/// `run_host_tool` over the declared tools (RFC 011 §6): the effect of each
/// call is derived from its declaration at stage 9; the definition's own
/// effect is the most restrictive reading.
pub fn run_host_tool_definition(
    declared: &[mimlys_core::host_tool::HostToolDeclaration],
) -> ToolDefinition {
    let names: Vec<String> = declared.iter().map(|d| d.name.clone()).collect();
    let max_args = declared.iter().map(|d| d.max_args).max().unwrap_or(0);
    ToolDefinition {
        name: "run_host_tool".into(),
        description: "Run one declared host tool under confinement in the workspace (offline); the result carries termination, exit_code, stdout and stderr.".into(),
        input: ParamSchema {
            params: vec![
                Param {
                    name: "tool".into(),
                    ty: ParamType::Enum { values: names },
                    optional: false,
                    description: "The declared tool.".into(),
                },
                Param {
                    name: "args".into(),
                    ty: ParamType::StringArray {
                        max_items: max_args.max(1),
                        max_chars: 400,
                    },
                    optional: true,
                    description: "Arguments the declaration allows.".into(),
                },
            ],
        },
        output: None,
        effect: mimlys_core::action::EffectView {
            kind: EffectKind::HostWrite,
            reversible: false,
            idempotent: false,
        },
        deadline_ms: mimlys_core::limits::LIM_H1_HOST_DEADLINE_MS,
        profile: ResourceProfile::Host,
        destination_arguments: vec![],
        provider: Provider::Builtin,
    }
}

impl WorkerTools {
    /// Over the task's tree, with the configured languages and the sandbox deadline (LIM-S2 or the configured value).
    pub fn new(
        builtin: BuiltinTools,
        worker: Arc<Worker>,
        task_id: TaskId,
        workspace_root: String,
        languages: Vec<String>,
        deadline_ms: i64,
    ) -> Self {
        Self {
            builtin,
            worker,
            task_id,
            workspace_root,
            languages,
            deadline_ms,
            host_tools: vec![],
            host_tools_available: false,
            fetch_worker: None,
        }
    }

    /// Adds the declared host tools; they are listed only when `available`.
    pub fn with_host_tools(
        mut self,
        declared: Vec<mimlys_core::host_tool::HostToolDeclaration>,
        available: bool,
        fetch_worker: Option<Arc<Worker>>,
    ) -> Self {
        self.host_tools = declared;
        self.host_tools_available = available;
        self.fetch_worker = fetch_worker;
        self
    }
}

impl ToolProvider for WorkerTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs = self.builtin.definitions();
        let mut script = run_script_definition(&self.languages);
        script.deadline_ms = self.deadline_ms;
        let mut wasm = run_wasm_definition();
        wasm.deadline_ms = self.deadline_ms;
        defs.push(script);
        defs.push(wasm);
        if self.host_tools_available && !self.host_tools.is_empty() {
            defs.push(run_host_tool_definition(&self.host_tools));
        }
        defs
    }

    fn execute<'a>(
        &'a self,
        action: &'a AuthorizedAction,
        context: ExecutionContext,
    ) -> BoxFuture<'a, ExecutionOutcome> {
        if !matches!(action.tool(), "run_script" | "run_wasm" | "run_host_tool") {
            return self.builtin.execute(action, context);
        }
        let args = action.arguments().clone();
        // The dependency step runs in the worker whose clause N is lifted (RFC 011 §8).
        let network_tool = action.tool() == "run_host_tool"
            && args
                .get("tool")
                .and_then(|v| v.as_str())
                .and_then(|name| self.host_tools.iter().find(|d| d.name == name))
                .is_some_and(|d| d.network);
        let worker = if network_tool {
            match &self.fetch_worker {
                Some(w) => Arc::clone(w),
                None => {
                    return Box::pin(async {
                        ExecutionOutcome {
                            termination: mimlys_core::action::Termination::HostFault {
                                detail: mimlys_core::action::HostFaultDetail::Unavailable,
                            },
                            failure: Some(mimlys_core::action::FailureCause::ToolUnavailable),
                            payload: serde_json::json!({"reason": "no worker for the dependency step"}),
                            stdout: Vec::new(),
                            stderr: Vec::new(),
                            notices: Vec::new(),
                            metrics: Default::default(),
                        }
                    });
                }
            }
        } else {
            Arc::clone(&self.worker)
        };
        let execution = Execution {
            task_id: self.task_id,
            execution_id: mimlys_core::ids::ExecutionId::new(&mimlys_core::ids::SystemIds),
            action_id: mimlys_core::ids::ActionId::new(&mimlys_core::ids::SystemIds),
            tool: action.tool().to_owned(),
            arguments: serde_json::Value::Object(args.clone()),
            effect: action.effect(),
            deadline_ms: context.deadline_ms,
            workspace_root: self.workspace_root.clone(),
            read_only: is_read_only(&args),
            replay: None,
        };
        let cancel = context.cancel.clone();
        Box::pin(async move {
            // A blocking thread does not inherit the step's span: carry it in,
            // so that the worker's lines and this execution's own are the step's.
            let span = tracing::Span::current();
            match tokio::task::spawn_blocking(move || {
                span.in_scope(|| worker.execute(execution, cancel))
            })
            .await
            {
                Ok(outcome) => outcome,
                Err(_) => ExecutionOutcome {
                    termination: mimlys_core::action::Termination::HostFault {
                        detail: mimlys_core::action::HostFaultDetail::Unavailable,
                    },
                    failure: Some(mimlys_core::action::FailureCause::SandboxFault {
                        detail: mimlys_core::action::HostFaultDetail::Unavailable,
                    }),
                    payload: serde_json::Value::Null,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    notices: Vec::new(),
                    metrics: Default::default(),
                },
            }
        })
    }
}
