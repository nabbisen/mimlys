use super::open::{Access, ReadError, open_regular, read_bounded};
use super::{Body, ToolError, ToolResult, int_arg, make_definition, path_param, workspace_path};
use mimlys_core::action::EffectKind;
use mimlys_core::bound::{Unit, serialized_size};
use mimlys_core::limits::{
    LIM_C2_EXCERPT_HEAD_CHARS, LIM_C2_EXCERPT_TAIL_CHARS, LIM_I19_RESULT_TOTAL_CHARS,
    LIM_W3_FILE_BYTES,
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
                    max: LIM_I19_RESULT_TOTAL_CHARS as i64,
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
    // The file is opened without blocking and is a regular file; what is read is bounded
    // by LIM-W3, the size of a file a tool writes: `total_chars` and `total_lines` are of
    // the whole file, so the read goes to its end, and a larger file is refused.
    let file = open_regular(&body.dir, path.as_str(), Access::Read).map_err(|e| {
        e.into_tool(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::new("not_found"),
            _ => ToolError::io("read_failed", e),
        })
    })?;
    let bytes = read_bounded(file, LIM_W3_FILE_BYTES).map_err(|e| match e {
        ReadError::TooLarge => ToolError {
            detail: "too_large",
            payload: json!({ "max_bytes": LIM_W3_FILE_BYTES }),
        },
        ReadError::Io(e) => ToolError::io("read_failed", &e),
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
    // What is returned is at most `max_chars` characters and, serialized with the
    // rest of the payload, at most LIM-I19: the escapes of a text (a newline or a
    // quote is two characters) and the other fields count, so the content is
    // cut, from its tail, to the longest that fits. It is a prefix: `truncated`,
    // `total_chars` and `line_range` say what was returned, and a following read
    // starts at the line after `line_range`.
    let payload_of = |chars: usize| {
        let content: String = from_line.chars().take(chars).collect();
        let lines_returned = content.lines().count().max(1);
        json!({
            "path": path.as_str(),
            "content": content,
            "truncated": chars < total_chars,
            "total_chars": total_chars,
            "line_range": [start_line, start_line + lines_returned - 1],
        })
    };
    let fits = |chars: usize| {
        serialized_size(&payload_of(chars), Unit::Chars) <= LIM_I19_RESULT_TOTAL_CHARS
    };
    let mut chars = total_chars.min(max_chars);
    if !fits(chars) {
        // The size grows with the number of characters kept: the largest that fits.
        let (mut lo, mut hi) = (0, chars);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        chars = lo;
    }
    if chars < total_chars {
        notices.push(Notice::ContentTruncated {
            total_chars: total_chars as u64,
        });
    }
    Ok((payload_of(chars), notices))
}
