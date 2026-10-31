use super::{Body, ToolError, ToolResult, int_arg, make_definition, path_param, workspace_path};
use mimlys_core::action::EffectKind;
use mimlys_core::limits::{
    LIM_C2_EXCERPT_CEILING_CHARS, LIM_C2_EXCERPT_HEAD_CHARS, LIM_C2_EXCERPT_TAIL_CHARS,
};
use mimlys_core::observation::{Notice, decode_lossy};
use mimlys_core::tool::{Param, ParamType, ToolDefinition};
use serde_json::json;

pub(super) fn definition() -> ToolDefinition {
    make_definition(
        "read_file",
        "Read a text file from the workspace, from an optional start line (1-based), up to max_chars characters. Fails with line_out_of_range when start_line is past the end.",
        vec![
            path_param("path", "Path of the file, relative to the workspace root."),
            Param {
                name: "start_line".into(),
                ty: ParamType::Integer {
                    min: 1,
                    max: i64::MAX,
                },
                optional: true,
                description: "First line to return (1-based).".into(),
            },
            Param {
                name: "max_chars".into(),
                ty: ParamType::Integer {
                    min: 1,
                    max: LIM_C2_EXCERPT_CEILING_CHARS as i64,
                },
                optional: true,
                description: "Maximum characters to return.".into(),
            },
        ],
        EffectKind::WorkspaceRead,
    )
}

pub(super) fn run(body: Body) -> ToolResult {
    let path = workspace_path(&body.args, "path")?;
    if path.is_root() {
        return Err(ToolError::new("is_directory"));
    }
    let start_line = int_arg(&body.args, "start_line", 1).max(1) as usize;
    let max_chars = int_arg(
        &body.args,
        "max_chars",
        (LIM_C2_EXCERPT_HEAD_CHARS + LIM_C2_EXCERPT_TAIL_CHARS) as i64,
    )
    .max(1) as usize;
    let bytes = body.dir.read(path.as_str()).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ToolError::new("not_found"),
        _ => ToolError::io("read_failed", &e),
    })?;
    let (text, replaced) = decode_lossy(&bytes);
    let mut notices = Vec::new();
    if replaced {
        notices.push(Notice::InvalidUtf8Replaced {
            stream: mimlys_core::observation::Stream::Stdout,
        });
    }
    let total_lines = text.lines().count();
    if start_line > total_lines.max(1) {
        return Err(ToolError {
            detail: "line_out_of_range",
            payload: json!({ "total_lines": total_lines }),
        });
    }
    let from_line: String = text
        .lines()
        .skip(start_line - 1)
        .collect::<Vec<_>>()
        .join("\n");
    let total_chars = from_line.chars().count();
    let (content, truncated) = if total_chars > max_chars {
        notices.push(Notice::ContentTruncated {
            total_chars: total_chars as u64,
        });
        (from_line.chars().take(max_chars).collect::<String>(), true)
    } else {
        (from_line, false)
    };
    let lines_returned = content.lines().count().max(1);
    let last_line = start_line + lines_returned - 1;
    Ok((
        json!({
            "path": path.as_str(),
            "content": content,
            "truncated": truncated,
            "total_chars": total_chars,
            "line_range": [start_line, last_line],
        }),
        notices,
    ))
}
