use super::*;
use mimlys_core::action::{FailureCause, Termination};
use mimlys_core::observation::Notice;
use mimlys_core::path::PlatformRules;
use mimlys_core::spi::ExecutionContext;
use mimlys_core::validate::permissive::PermissiveHooks;
use mimlys_core::validate::{Outcome, ValidationContext, validate};
use serde_json::json;
use std::fs;

struct Fixture {
    root: tempfile::TempDir,
    tools: BuiltinTools,
    definitions: Vec<ToolDefinition>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("src/nested")).unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            "fn sum() {}\nfn main() {\n    sum();\n}\n",
        )
        .unwrap();
        fs::write(
            root.path().join("src/nested/deep.txt"),
            "needle here\nnothing\nneedle again\n",
        )
        .unwrap();
        fs::write(root.path().join("README.md"), "# readme\n").unwrap();
        let tools = BuiltinTools::open(root.path()).unwrap();
        Self {
            root,
            tools,
            definitions: BuiltinTools::all_definitions(),
        }
    }

    /// Runs the whole path: strict parse, validator, execution — as the loop will.
    async fn call(&self, tool: &str, args: serde_json::Value) -> ExecutionOutcome {
        let hooks = PermissiveHooks;
        let ctx = ValidationContext {
            task_running: true,
            pending_user_item: false,
            tools: &self.definitions,
            rules: PlatformRules::current(),
            grants: &hooks,
            effects: &hooks,
            budgets: &hooks,
        };
        let raw = serde_json::to_vec(&json!({"decision": "act", "tool": tool, "arguments": args}))
            .unwrap();
        match validate(&ctx, &raw) {
            Outcome::Authorized(action) => {
                self.tools
                    .execute(
                        &action,
                        ExecutionContext {
                            deadline_ms: action.deadline_ms(),
                            cancel: Cancellation::new(),
                            redactor: Default::default(),
                        },
                    )
                    .await
            }
            other => panic!("not authorized: {other:?}"),
        }
    }
}

fn detail(outcome: &ExecutionOutcome) -> &str {
    match &outcome.failure {
        Some(FailureCause::ToolError { detail }) => detail,
        other => panic!("expected tool error, got {other:?}"),
    }
}

fn paths(outcome: &ExecutionOutcome) -> Vec<String> {
    outcome.payload["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_owned())
        .collect()
}

/// REQ-TOOL-003: the six definitions are valid, snake_case and within every bound.
#[test]
fn req_tool_003_definitions_are_valid() {
    let defs = BuiltinTools::all_definitions();
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "read_file",
            "write_file",
            "replace_in_file",
            "list_dir",
            "search_files",
            "delete_path"
        ]
    );
    for d in &defs {
        d.check().unwrap();
        assert_eq!(d.profile, ResourceProfile::Builtin);
    }
    mimlys_core::tool::render_definitions(&defs).unwrap();
}

/// REQ-MDL-009 (RFC 012): the golden functions of every built-in tool (the
/// six file tools, `run_script`, `run_wasm`, `run_host_tool`): the same
/// descriptors give these bytes, so a change to a tool's descriptor or to the
/// generation is visible in review. The pinned definitions are not touched.
#[test]
fn req_mdl_009_golden_functions_of_every_builtin_tool() {
    let mut defs = BuiltinTools::all_definitions();
    defs.push(crate::tools::run_script_definition(&[
        "javascript".to_owned(),
        "python".to_owned(),
    ]));
    defs.push(crate::tools::run_wasm_definition());
    let declaration: mimlys_core::host_tool::HostToolDeclaration = serde_json::from_value(
        json!({"name": "cargo_test", "command": "cargo", "effect": "workspace_write", "max_args": 2}),
    )
    .unwrap();
    defs.push(crate::tools::run_host_tool_definition(&[declaration]));
    let before: Vec<String> = defs
        .iter()
        .map(|d| serde_json::to_string(d).unwrap())
        .collect();
    let offer = mimlys_core::native::offer(&defs);
    assert!(offer.unavailable.is_empty(), "{:?}", offer.unavailable);
    let actual = serde_json::to_string_pretty(&mimlys_core::native::tools_json(&offer.functions))
        .unwrap()
        + "\n";
    assert_eq!(
        actual,
        include_str!("native_functions.golden.json"),
        "the generated functions changed; review the golden file"
    );
    let again = serde_json::to_string_pretty(&mimlys_core::native::tools_json(
        &mimlys_core::native::offer(&defs).functions,
    ))
    .unwrap()
        + "\n";
    assert_eq!(actual, again, "the same input, the same bytes");
    let after: Vec<String> = defs
        .iter()
        .map(|d| serde_json::to_string(d).unwrap())
        .collect();
    assert_eq!(
        before, after,
        "generation leaves the definitions as they were"
    );
}

/// REQ-TOOL-003 / ED §11: `read_file` returns content, ranges, truncation notices, and fails past the end.
#[tokio::test]
async fn req_tool_003_read_file() {
    let f = Fixture::new();
    let o = f.call("read_file", json!({"path": "src/lib.rs"})).await;
    assert_eq!(o.termination, Termination::Exited { exit_code: 0 });
    assert_eq!(
        o.payload["content"],
        "fn sum() {}\nfn main() {\n    sum();\n}"
    );
    assert_eq!(o.payload["truncated"], false);
    assert_eq!(o.payload["line_range"], json!([1, 4]));
    let o = f
        .call(
            "read_file",
            json!({"path": "src/lib.rs", "start_line": 2, "max_chars": 5}),
        )
        .await;
    assert_eq!(o.payload["content"], "fn ma");
    assert_eq!(o.payload["truncated"], true);
    assert_eq!(o.payload["line_range"], json!([2, 2]));
    assert!(
        o.notices
            .iter()
            .any(|n| matches!(n, Notice::ContentTruncated { .. }))
    );
    let o = f
        .call("read_file", json!({"path": "src/lib.rs", "start_line": 4}))
        .await;
    assert_eq!(o.payload["line_range"], json!([4, 4]));
    let o = f
        .call("read_file", json!({"path": "src/lib.rs", "start_line": 99}))
        .await;
    assert_eq!(detail(&o), "line_out_of_range");
    assert_eq!(o.payload["total_lines"], 4);
    assert_eq!(
        detail(&f.call("read_file", json!({"path": "missing.txt"})).await),
        "not_found"
    );
    assert_eq!(
        detail(&f.call("read_file", json!({"path": "src"})).await),
        "is_directory"
    );
}

/// REQ-TOOL-003 / ED §11: `write_file` creates and overwrites, creates missing parents and reports them.
#[tokio::test]
async fn req_tool_003_write_file() {
    let f = Fixture::new();
    let o = f
        .call(
            "write_file",
            json!({"path": "src/new.rs", "content": "héllo"}),
        )
        .await;
    assert_eq!(o.payload["created"], true);
    assert_eq!(o.payload["bytes_written"], 6);
    assert_eq!(o.payload["parents_created"], 0);
    assert_eq!(
        fs::read_to_string(f.root.path().join("src/new.rs")).unwrap(),
        "héllo"
    );
    let o = f
        .call("write_file", json!({"path": "src/new.rs", "content": "x"}))
        .await;
    assert_eq!(o.payload["created"], false);
    let o = f
        .call(
            "write_file",
            json!({"path": "deep/er/new.rs", "content": "x"}),
        )
        .await;
    assert_eq!(o.payload["created"], true);
    assert_eq!(o.payload["parents_created"], 2);
    assert_eq!(
        fs::read_to_string(f.root.path().join("deep/er/new.rs")).unwrap(),
        "x"
    );
}

/// REQ-TOOL-003 / ED §11: `replace_in_file` enforces the match count and replaces all with 0.
#[tokio::test]
async fn req_tool_003_replace_in_file() {
    let f = Fixture::new();
    let o = f
        .call(
            "replace_in_file",
            json!({"path": "src/lib.rs", "find": "fn main", "replace": "pub fn main"}),
        )
        .await;
    assert_eq!(o.payload["replaced"], 1);
    assert!(
        fs::read_to_string(f.root.path().join("src/lib.rs"))
            .unwrap()
            .contains("pub fn main")
    );
    let o = f
        .call(
            "replace_in_file",
            json!({"path": "src/lib.rs", "find": "absent", "replace": ""}),
        )
        .await;
    assert_eq!(detail(&o), "no_match");
    let o = f
        .call(
            "replace_in_file",
            json!({"path": "src/lib.rs", "find": "sum", "replace": "add"}),
        )
        .await;
    assert_eq!(detail(&o), "ambiguous");
    assert_eq!(o.payload["matches"], 2);
    let o = f
        .call(
            "replace_in_file",
            json!({"path": "src/lib.rs", "find": "sum", "replace": "add", "occurrences": 0}),
        )
        .await;
    assert_eq!(o.payload["replaced"], 2);
}

/// REQ-TOOL-003 / ED §11: `list_dir` lists sorted entries to the requested depth; `search_files` finds lines.
#[tokio::test]
async fn req_tool_003_list_dir_and_search_files() {
    let f = Fixture::new();
    let o = f.call("list_dir", json!({"path": "."})).await;
    assert_eq!(paths(&o), ["README.md", "src"]);
    let o = f.call("list_dir", json!({"path": "src", "depth": 2})).await;
    assert_eq!(
        paths(&o),
        ["src/lib.rs", "src/nested", "src/nested/deep.txt"]
    );
    assert_eq!(
        detail(&f.call("list_dir", json!({"path": "README.md"})).await),
        "not_a_directory"
    );
    let o = f
        .call(
            "search_files",
            json!({"pattern": "needle", "glob": "**/*.txt"}),
        )
        .await;
    let m = o.payload["matches"].as_array().unwrap();
    assert_eq!(m.len(), 2);
    assert_eq!(m[0]["path"], "src/nested/deep.txt");
    assert_eq!(m[0]["line"], 1);
    assert!(m[0].get("truncated").is_none());
    assert!(o.notices.is_empty());
    let o = f
        .call(
            "search_files",
            json!({"pattern": "needle", "max_results": 1}),
        )
        .await;
    assert_eq!(o.payload["truncated"], true);
    assert!(
        o.notices
            .iter()
            .any(|n| matches!(n, Notice::EntriesTruncated { total: 2 }))
    );
    fs::write(
        f.root.path().join("long.txt"),
        format!("{}needle\n", "x".repeat(500)),
    )
    .unwrap();
    let o = f
        .call(
            "search_files",
            json!({"pattern": "needle", "glob": "long.txt"}),
        )
        .await;
    assert_eq!(o.payload["matches"][0]["truncated"], true);
    assert_eq!(
        detail(&f.call("search_files", json!({"pattern": "("})).await),
        "invalid_pattern"
    );
}

/// REQ-TOOL-003 / ED §11: `delete_path` refuses the root and non-empty directories without `recursive`.
#[tokio::test]
async fn req_tool_003_delete_path() {
    let f = Fixture::new();
    assert_eq!(
        detail(&f.call("delete_path", json!({"path": "."})).await),
        "is_root"
    );
    assert_eq!(
        detail(&f.call("delete_path", json!({"path": "src"})).await),
        "not_empty"
    );
    let o = f.call("delete_path", json!({"path": "README.md"})).await;
    assert_eq!(o.payload["deleted"], 1);
    let o = f
        .call("delete_path", json!({"path": "src", "recursive": true}))
        .await;
    assert_eq!(o.payload["deleted"], 4);
    assert!(!f.root.path().join("src").exists());
    assert_eq!(
        detail(&f.call("delete_path", json!({"path": "src"})).await),
        "not_found"
    );
}

/// REQ-TOOL-006 / REQ-TOOL-007 / INV-04 (external design §21.2): the
/// deadline ends an execution with `deadline`; cancellation is observed.
/// INV-04's deadline clause for a built-in (non-sandbox) execution: a search
/// over a large workspace given 1 ms ends `deadline`/`timeout`.
#[tokio::test]
async fn req_tool_006_007_deadline_and_cancellation() {
    let f = Fixture::new();
    let hooks = PermissiveHooks;
    let ctx = ValidationContext {
        task_running: true,
        pending_user_item: false,
        tools: &f.definitions,
        rules: PlatformRules::current(),
        grants: &hooks,
        effects: &hooks,
        budgets: &hooks,
    };
    let raw = serde_json::to_vec(
        &json!({"decision": "act", "tool": "search_files", "arguments": {"pattern": "x"}}),
    )
    .unwrap();
    let Outcome::Authorized(action) = validate(&ctx, &raw) else {
        panic!()
    };
    let cancel = Cancellation::new();
    cancel.cancel();
    let o = f
        .tools
        .execute(
            &action,
            ExecutionContext {
                deadline_ms: 30_000,
                cancel,
                redactor: Default::default(),
            },
        )
        .await;
    assert_eq!(o.termination, Termination::Cancelled);
    // A 1 ms deadline fires long before a search over a large workspace completes.
    for i in 0..3000 {
        let line = format!("line {i} with some text to scan\n").repeat(100);
        fs::write(f.root.path().join(format!("src/nested/f{i}.txt")), line).unwrap();
    }
    let o = f
        .tools
        .execute(
            &action,
            ExecutionContext {
                deadline_ms: 1,
                cancel: Cancellation::new(),
                redactor: Default::default(),
            },
        )
        .await;
    assert_eq!(o.termination, Termination::Deadline);
    assert_eq!(o.failure, Some(FailureCause::Timeout));
}

/// REQ-SEC-012 (handoff 008c), REQ-TOOL-003: the pinned definition hash of every
/// built-in file tool is the one 0.9.0 computed (tasks of 0.9.0 pin these), whatever
/// `sha2` the build uses.
#[test]
fn req_tool_003_golden_definition_hashes_of_0_9_0() {
    let expected = [
        (
            "read_file",
            "6ad0178877631ad7a305d38e2bad40737f894c291a11a4e71bad31d1881d5335",
        ),
        (
            "write_file",
            "a9b52ef2ceb29485a876b043a460419ac904f1a8a01791b3d6c1b83a267416a4",
        ),
        (
            "replace_in_file",
            "1113facd7c6a6d509aec02494a187d8ae830769344daef5c44ecea05170c5579",
        ),
        (
            "list_dir",
            "2da1196f44aa4bcaa7f6d6b8187ddfb2b3d11538a45b0c54f9f32e653b003104",
        ),
        (
            "search_files",
            "5bb3e5ce184e46a0e55ca0874e07f4cdd084c02979e828c9059737ae6cb040ff",
        ),
        (
            "delete_path",
            "80370161a0ed0c027277d6b7ebb15fb9308d3b8f7f0f4149193f7c7620a0db68",
        ),
    ];
    let definitions = crate::builtin::BuiltinTools::all_definitions();
    assert_eq!(definitions.len(), expected.len());
    for (definition, (name, hash)) in definitions.iter().zip(expected) {
        assert_eq!(definition.name, name);
        let actual = mimlys_core::hash::hash_canonical(&serde_json::to_value(definition).unwrap());
        assert_eq!(actual.as_str(), hash, "{name}");
    }
}

/// REQ-TOOL-003: a tool refuses a directory with `is_directory` and its hint, whatever it
/// was going to do with it.
#[tokio::test]
async fn req_tool_003_a_directory_is_refused_by_the_tools_that_open_files() {
    let f = Fixture::new();
    for (tool, args) in [
        ("read_file", json!({"path": "src"})),
        ("write_file", json!({"path": "src", "content": "x"})),
        (
            "replace_in_file",
            json!({"path": "src", "find": "a", "replace": "b"}),
        ),
    ] {
        let o = f.call(tool, args).await;
        assert_eq!(detail(&o), "is_directory", "{tool}");
        assert!(
            o.payload["hint"]
                .as_str()
                .is_some_and(|h| h.contains("list_dir"))
        );
    }
    assert!(f.root.path().join("src").is_dir(), "nothing was changed");
}

/// REQ-TOOL-003: `read_file` stops at LIM-W3 (the size of a file a tool writes): a file of
/// that size is read, one byte more is refused as `too_large`, and so is its replacement
/// and its search (skipped and counted).
#[tokio::test]
async fn req_tool_003_a_read_is_bounded_by_the_file_size_limit() {
    let f = Fixture::new();
    let limit = mimlys_core::limits::LIM_W3_FILE_BYTES;
    let at = f.root.path().join("at_limit.txt");
    let over = f.root.path().join("over_limit.txt");
    fs::write(&at, vec![b'a'; limit]).unwrap();
    fs::write(&over, vec![b'a'; limit + 1]).unwrap();

    let o = f.call("read_file", json!({"path": "at_limit.txt"})).await;
    assert_eq!(o.termination, Termination::Exited { exit_code: 0 });
    assert_eq!(o.payload["total_chars"], limit);
    let o = f.call("read_file", json!({"path": "over_limit.txt"})).await;
    assert_eq!(detail(&o), "too_large");
    assert_eq!(o.payload["max_bytes"], limit);
    let o = f
        .call(
            "replace_in_file",
            json!({"path": "over_limit.txt", "find": "a", "replace": "b", "occurrences": 0}),
        )
        .await;
    assert_eq!(detail(&o), "too_large");
    assert_eq!(fs::metadata(&over).unwrap().len(), limit as u64 + 1);
    let o = f
        .call(
            "search_files",
            json!({"pattern": "zzz", "glob": "over_limit.txt"}),
        )
        .await;
    assert!(o.notices.iter().any(|n| matches!(
        n,
        Notice::EntriesSkipped {
            reason: mimlys_core::observation::SkipReason::TooLarge,
            count: 1
        }
    )));
}

#[cfg(unix)]
mod special_files {
    use rustix::fs::{CWD, FileType, Mode, mknodat};
    use std::time::Duration;

    pub(super) fn make_fifo(path: &std::path::Path) {
        mknodat(CWD, path, FileType::Fifo, Mode::from_raw_mode(0o600), 0).unwrap();
    }

    /// Runs `body` on a thread of its own, with a runtime of its own that is dropped on that
    /// thread (a runtime waits for its blocking threads when it is dropped), and fails when
    /// it has not finished within LIM-O1: a test that must fail does so, it does not hang.
    pub(super) fn finishes_within_the_stop_limit<T: Send + 'static>(
        body: impl FnOnce(&tokio::runtime::Runtime) -> T + Send + 'static,
    ) -> T {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let result = body(&runtime);
            drop(runtime);
            let _ = sender.send(result);
        });
        receiver
            .recv_timeout(Duration::from_millis(
                mimlys_core::limits::LIM_O1_STOP_MS as u64,
            ))
            .expect("the tools did not finish, and the runtime did not exit, within LIM-O1")
    }
}

/// REQ-TOOL-003, REQ-TOOL-006, REQ-CLI-005: a tool given a named pipe or a socket refuses it
/// at once with `not_a_file` and its hint, and the runtime that ran it exits within LIM-O1.
/// The file is left as it was. A search skips both and counts them as unreadable.
#[cfg(unix)]
#[test]
fn req_tool_003_a_named_pipe_or_a_socket_is_refused_and_the_runtime_exits_within_lim_o1() {
    use special_files::*;
    let seen = finishes_within_the_stop_limit(|runtime| {
        let f = Fixture::new();
        make_fifo(&f.root.path().join("pipe"));
        let listener = std::os::unix::net::UnixListener::bind(f.root.path().join("sock")).unwrap();
        let mut seen = Vec::new();
        for (path, kind) in [("pipe", "fifo"), ("sock", "socket")] {
            for (tool, args) in [
                ("read_file", json!({"path": path})),
                ("write_file", json!({"path": path, "content": "x"})),
                (
                    "replace_in_file",
                    json!({"path": path, "find": "a", "replace": "b"}),
                ),
            ] {
                let o = runtime.block_on(f.call(tool, args));
                seen.push((tool, kind, detail(&o).to_owned(), o.payload.clone()));
            }
        }
        // The walk of a search skips both and counts them; a listing shows them; a deletion
        // removes them: none of these opens the file.
        let o = runtime.block_on(f.call("search_files", json!({"pattern": "needle"})));
        assert!(
            o.notices.iter().any(|n| matches!(
                n,
                Notice::EntriesSkipped {
                    reason: mimlys_core::observation::SkipReason::Unreadable,
                    count: 2
                }
            )),
            "{:?}",
            o.notices
        );
        assert_eq!(o.payload["matches"].as_array().unwrap().len(), 2);
        let o = runtime.block_on(f.call("list_dir", json!({"path": "."})));
        assert!(paths(&o).contains(&"pipe".to_owned()));
        for path in ["pipe", "sock"] {
            assert!(
                fs::symlink_metadata(f.root.path().join(path)).is_ok(),
                "{path} is as it was"
            );
        }
        for path in ["pipe", "sock"] {
            let o = runtime.block_on(f.call("delete_path", json!({"path": path})));
            assert_eq!(o.payload["deleted"], 1);
        }
        drop(listener);
        seen
    });
    assert_eq!(seen.len(), 6);
    for (tool, kind, detail, payload) in seen {
        assert_eq!(detail, "not_a_file", "{tool} on a {kind}");
        assert_eq!(payload["type"], kind, "{tool}");
        assert!(
            payload["hint"].as_str().is_some_and(|h| !h.is_empty()),
            "{tool}"
        );
    }
}

/// REQ-TOOL-006: the open itself does not wait. A named pipe opened for reading with no
/// writer, and for writing with no reader, returns at once: the handle for reading (which
/// the path look-up before it would not let through), and the operating system's refusal for
/// writing. This is what stands behind the look-up when the file changes in between.
#[cfg(unix)]
#[test]
fn req_tool_006_the_open_of_a_named_pipe_does_not_wait() {
    use super::open::{Access, open_handle};
    use special_files::*;
    let (read, write) = finishes_within_the_stop_limit(|_runtime| {
        let root = tempfile::tempdir().unwrap();
        make_fifo(&root.path().join("pipe"));
        let dir = Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap();
        let read = open_handle(&dir, "pipe", Access::Read)
            .map(|file| cap_std::fs::FileTypeExt::is_fifo(&file.metadata().unwrap().file_type()));
        let write = open_handle(&dir, "pipe", Access::Write).map(|_| ());
        (
            read.map_err(|e| e.kind()),
            write.map_err(|e| e.raw_os_error()),
        )
    });
    assert_eq!(read, Ok(true), "opened for reading, and it is a named pipe");
    assert_eq!(
        write,
        Err(Some(rustix::io::Errno::NXIO.raw_os_error())),
        "no reader: refused at once"
    );
}
