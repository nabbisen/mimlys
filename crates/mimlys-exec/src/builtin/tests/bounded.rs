//! Every tool's result is bounded as the model reads it (handoff 013c, item 1;
//! LIM-I19; REQ-CTX-006): `read_file` takes at most LIM-I19 characters and
//! returns a prefix that fits serialized, `list_dir` and `search_files` drop
//! whole trailing entries and say so, and no path is ever cut.

use super::*;
use mimlys_core::bound::{Unit, serialized_size};
use mimlys_core::limits::{LIM_C2_EXCERPT_CHARS_RANGE, LIM_I19_RESULT_TOTAL_CHARS};

const LIM: usize = LIM_I19_RESULT_TOTAL_CHARS;

fn size(outcome: &ExecutionOutcome) -> usize {
    serialized_size(&outcome.payload, Unit::Chars)
}

/// The validator's verdict on a `read_file` call, as the model's decision reaches it.
fn verdict(f: &Fixture, args: serde_json::Value) -> Outcome {
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
    let raw =
        serde_json::to_vec(&json!({"decision": "act", "tool": "read_file", "arguments": args}))
            .unwrap();
    validate(&ctx, &raw)
}

/// LIM-I19, LIM-C2: the schema's maximum of `max_chars`, which is what the model
/// is shown, is LIM-I19 (the range of LIM-C2 ends there): a request for one
/// character more is refused by the validator, never run; LIM-I19 itself is run.
#[test]
fn req_ctx_006_read_file_refuses_max_chars_above_lim_i19() {
    let f = Fixture::new();
    assert_eq!(LIM_C2_EXCERPT_CHARS_RANGE, (500, LIM));
    let definition = f
        .definitions
        .iter()
        .find(|d| d.name == "read_file")
        .unwrap();
    let max = definition
        .input
        .params
        .iter()
        .find(|p| p.name == "max_chars")
        .map(|p| match p.ty {
            ParamType::Integer { max, .. } => max,
            _ => panic!("not an integer"),
        })
        .unwrap();
    assert_eq!(max, LIM as i64);
    assert!(matches!(
        verdict(&f, json!({"path": "README.md", "max_chars": LIM})),
        Outcome::Authorized(_)
    ));
    match verdict(&f, json!({"path": "README.md", "max_chars": LIM + 1})) {
        Outcome::Rejected { rejection, .. } => {
            assert_eq!(
                rejection.code,
                mimlys_core::rejection::RejectionCode::InvalidArgument
            )
        }
        other => panic!("not refused: {other:?}"),
    }
}

/// LIM-I19: even the largest `max_chars` returns a payload that fits as the
/// model reads it: the escapes of a text count, so what is returned is a shorter
/// prefix, `truncated` says so, `line_range` is of what was returned, and the
/// notice carries the whole length.
#[tokio::test]
async fn req_ctx_006_read_file_returns_a_prefix_that_fits_serialized() {
    let f = Fixture::new();
    // Every quote and every newline is two characters in the serialization.
    let text: String = (0..LIM).map(|i| format!("\"{}\n", i % 10)).collect();
    fs::write(f.root.path().join("quotes.txt"), &text).unwrap();
    let o = f
        .call("read_file", json!({"path": "quotes.txt", "max_chars": LIM}))
        .await;
    assert_eq!(o.termination, Termination::Exited { exit_code: 0 });
    assert!(size(&o) <= LIM, "{}", size(&o));
    let content = o.payload["content"].as_str().unwrap();
    assert!(content.chars().count() < LIM, "the escapes are counted");
    assert!(text.starts_with(content), "a prefix, never a rewrite");
    assert_eq!(o.payload["truncated"], true);
    let returned = content.lines().count();
    assert_eq!(o.payload["line_range"], json!([1, returned]));
    let total = text.trim_end_matches('\n').chars().count();
    assert_eq!(o.payload["total_chars"], total);
    assert!(o.notices.iter().any(
        |n| matches!(n, Notice::ContentTruncated { total_chars } if *total_chars == total as u64)
    ));
    // The largest prefix that fits: one character more would not.
    let one_more: String = text.chars().take(content.chars().count() + 1).collect();
    let bigger = json!({
        "path": "quotes.txt", "content": one_more, "truncated": true,
        "total_chars": total, "line_range": [1, one_more.lines().count().max(1)],
    });
    assert!(serialized_size(&bigger, Unit::Chars) > LIM);
    // A plain file longer than the maximum is cut as well, and a short one is
    // untouched.
    fs::write(f.root.path().join("plain.txt"), "a".repeat(LIM + 100)).unwrap();
    let o = f
        .call("read_file", json!({"path": "plain.txt", "max_chars": LIM}))
        .await;
    assert!(size(&o) <= LIM);
    assert_eq!(o.payload["truncated"], true);
    let o = f.call("read_file", json!({"path": "src/lib.rs"})).await;
    assert_eq!(o.payload["truncated"], false);
    assert!(o.notices.is_empty());
}

/// LIM-I19, LIM-I7: a listing over LIM-I19 is `truncated`, with whole entries
/// only (every path is a path that exists, none is cut), the entries that are
/// kept are the first ones in order, and the notice carries the total.
#[tokio::test]
async fn req_ctx_006_a_listing_over_lim_i19_drops_trailing_whole_entries() {
    let f = Fixture::new();
    let dir = f.root.path().join("many");
    fs::create_dir_all(&dir).unwrap();
    let mut names = Vec::new();
    for i in 0..300 {
        let name = format!("{i:03}-{}", "n".repeat(60));
        fs::write(dir.join(&name), "").unwrap();
        names.push(format!("many/{name}"));
    }
    let o = f.call("list_dir", json!({"path": "many"})).await;
    // Under LIM-I7 entries, over LIM-I19 serialized.
    assert!(names.len() < mimlys_core::limits::LIM_I7_ENTRIES);
    assert!(size(&o) <= LIM, "{}", size(&o));
    assert_eq!(o.payload["truncated"], true);
    let listed = paths(&o);
    assert!(!listed.is_empty() && listed.len() < names.len());
    assert_eq!(
        listed,
        names[..listed.len()],
        "whole entries, in order, from the start"
    );
    for path in &listed {
        assert!(f.root.path().join(path).exists(), "{path} is not a path");
    }
    assert!(
        o.notices
            .iter()
            .any(|n| matches!(n, Notice::EntriesTruncated { total: 300 }))
    );
    // The most that fits was kept: the next entry would not have.
    let next = json!({"path": names[listed.len()], "type": "file", "size": 0});
    let mut all = o.payload["entries"].as_array().unwrap().clone();
    all.push(next);
    let mut bigger = o.payload.clone();
    bigger["entries"] = json!(all);
    assert!(serialized_size(&bigger, Unit::Chars) > LIM);
    // A listing that fits is not truncated.
    let o = f.call("list_dir", json!({"path": "src"})).await;
    assert_eq!(o.payload["truncated"], false);
    assert!(o.notices.is_empty());
}

/// LIM-I19: matches are dropped whole from the end, `truncated` is set and the
/// total is reported; a result that fits is untouched.
#[tokio::test]
async fn req_ctx_006_search_matches_over_lim_i19_drop_trailing_whole_matches() {
    let f = Fixture::new();
    let long_line = format!("needle {}", "z".repeat(350));
    let content: String = (0..200).map(|_| format!("{long_line}\n")).collect();
    fs::write(f.root.path().join("hay.txt"), content).unwrap();
    let o = f
        .call(
            "search_files",
            json!({"pattern": "needle", "glob": "hay.txt", "max_results": 200}),
        )
        .await;
    assert!(size(&o) <= LIM, "{}", size(&o));
    assert_eq!(o.payload["truncated"], true);
    let matches = o.payload["matches"].as_array().unwrap();
    assert!(!matches.is_empty() && matches.len() < 200);
    for (i, m) in matches.iter().enumerate() {
        assert_eq!(m["line"], i + 1, "the first matches, in order");
        assert_eq!(m["text"], long_line);
    }
    assert!(
        o.notices
            .iter()
            .any(|n| matches!(n, Notice::EntriesTruncated { total: 200 }))
    );
    let o = f
        .call(
            "search_files",
            json!({"pattern": "needle", "glob": "**/*.txt", "max_results": 3}),
        )
        .await;
    assert!(size(&o) <= LIM);
    assert_eq!(o.payload["matches"].as_array().unwrap().len(), 3);
}
