use super::*;
use mimlys_core::action::{FailureCause, Termination};
use mimlys_core::observation::Notice;
#[cfg(unix)]
use mimlys_core::observation::SkipReason;
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
        "read_failed"
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

/// REQ-TOOL-004 / THR-03 / THR-27: a symbolic link that leaves the workspace is refused at open time;
/// listings show links as `symlink` without a target; searches skip them and count the skips.
/// REQ-SEC-005 (external design §11), symbolic links: reading or writing
/// through a link to a file or directory outside the workspace fails and the
/// outside file is neither read nor created. (Unix only: the test creates
/// links with `std::os::unix`.)
/// REQ-TEST-005 (external design §6.3), symbolic links.
/// REQ-SEC-013 (hostile corpus, repository guards §3a), hostile paths: symbolic links.
/// REQ-PRIV-001 (external design §14.2), minimization: the content of a file outside
/// the workspace (`top secret`) does not enter an observation through a link, a
/// listing or a search.
#[cfg(unix)]
#[tokio::test]
async fn req_tool_004_symlink_escape_is_refused() {
    let f = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.txt"), "top secret").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        f.root.path().join("link.txt"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), f.root.path().join("linkdir")).unwrap();
    let o = f.call("read_file", json!({"path": "link.txt"})).await;
    assert!(
        o.failure.is_some(),
        "reading through an escaping link must fail: {o:?}"
    );
    assert_ne!(o.payload["content"], "top secret");
    let o = f
        .call("read_file", json!({"path": "linkdir/secret.txt"}))
        .await;
    assert!(o.failure.is_some());
    let o = f
        .call(
            "write_file",
            json!({"path": "linkdir/planted.txt", "content": "x"}),
        )
        .await;
    assert!(o.failure.is_some());
    assert!(!outside.path().join("planted.txt").exists());
    let o = f.call("list_dir", json!({"path": ".", "depth": 3})).await;
    let entries = o.payload["entries"].as_array().unwrap();
    let link = entries.iter().find(|e| e["path"] == "link.txt").unwrap();
    assert_eq!(
        link["type"], "symlink",
        "links are listed as such, without their target"
    );
    assert!(link.get("target").is_none());
    assert!(
        !paths(&o).iter().any(|p| p.starts_with("linkdir/")),
        "a link is never descended"
    );
    let o = f.call("search_files", json!({"pattern": "secret"})).await;
    assert!(
        o.payload["matches"].as_array().unwrap().is_empty(),
        "a search never follows a link"
    );
    assert!(o.notices.iter().any(|n| matches!(
        n,
        Notice::EntriesSkipped {
            reason: SkipReason::Symlink,
            count: 2
        }
    )));
    // Deleting a link removes the link, not its target.
    let o = f.call("delete_path", json!({"path": "link.txt"})).await;
    assert_eq!(o.payload["deleted"], 1);
    assert!(outside.path().join("secret.txt").exists());
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
