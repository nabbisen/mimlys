use super::open::{Access, ReadError, open_regular, read_bounded};
use super::{
    Body, ToolError, ToolResult, int_arg, make_definition, path_param, str_arg, workspace_path,
};
use mimlys_core::action::EffectKind;
use mimlys_core::limits::LIM_W3_FILE_BYTES;
use mimlys_core::tool::{Param, ParamType, ToolDefinition};
use serde_json::json;
use std::io::Write;

pub(super) fn definition() -> ToolDefinition {
    make_definition(
        "replace_in_file",
        "Replace exact text in a workspace file. Fails when `find` matches a different number of times than `occurrences` (default 1; 0 means every occurrence).",
        vec![
            path_param("path", "Path of the file, relative to the workspace root."),
            Param {
                name: "find".into(),
                ty: ParamType::String {
                    min_chars: 1,
                    max_chars: LIM_W3_FILE_BYTES,
                    max_bytes: Some(LIM_W3_FILE_BYTES),
                },
                optional: false,
                description: "Exact text to find.".into(),
            },
            Param {
                name: "replace".into(),
                ty: ParamType::String {
                    min_chars: 0,
                    max_chars: LIM_W3_FILE_BYTES,
                    max_bytes: Some(LIM_W3_FILE_BYTES),
                },
                optional: false,
                description: "Replacement text.".into(),
            },
            Param {
                name: "occurrences".into(),
                ty: ParamType::Integer {
                    min: 0,
                    max: 100_000,
                },
                optional: true,
                description: "Expected number of matches; 0 replaces all.".into(),
            },
        ],
        EffectKind::WorkspaceWrite,
    )
}

pub(super) fn run(body: Body) -> ToolResult {
    let path = workspace_path(&body.args, "path")?;
    let find = str_arg(&body.args, "find").ok_or(ToolError::new("missing_find"))?;
    let replace = str_arg(&body.args, "replace").ok_or(ToolError::new("missing_replace"))?;
    let occurrences = int_arg(&body.args, "occurrences", 1).max(0) as usize;
    let file = open_regular(&body.dir, path.as_str(), Access::Read).map_err(|e| {
        e.into_tool(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::new("not_found"),
            _ => ToolError::io("read_failed", e),
        })
    })?;
    // A file over LIM-W3 could not be written back whole: it is refused before it is read.
    let bytes = read_bounded(file, LIM_W3_FILE_BYTES).map_err(|e| match e {
        ReadError::TooLarge => ToolError::new("too_large"),
        ReadError::Io(e) => ToolError::io("read_failed", &e),
    })?;
    let text = String::from_utf8(bytes).map_err(|_| ToolError::new("not_utf8"))?;
    let count = text.matches(find).count();
    if count == 0 {
        return Err(ToolError::new("no_match"));
    }
    if occurrences != 0 && count != occurrences {
        return Err(ToolError {
            detail: "ambiguous",
            payload: json!({ "matches": count, "expected": occurrences }),
        });
    }
    let replaced = text.replace(find, replace);
    if replaced.len() > LIM_W3_FILE_BYTES {
        return Err(ToolError::new("too_large"));
    }
    let mut file = open_regular(&body.dir, path.as_str(), Access::Write)
        .map_err(|e| e.into_tool(|e| ToolError::io("write_failed", e)))?;
    file.set_len(0)
        .and_then(|()| file.write_all(replaced.as_bytes()))
        .map_err(|e| ToolError::io("write_failed", &e))?;
    Ok((
        json!({ "path": path.as_str(), "replaced": count }),
        Vec::new(),
    ))
}
