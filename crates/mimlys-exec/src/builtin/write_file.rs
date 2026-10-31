use super::{Body, ToolError, ToolResult, make_definition, path_param, str_arg, workspace_path};
use mimlys_core::action::EffectKind;
use mimlys_core::limits::LIM_W3_FILE_BYTES;
use mimlys_core::tool::{Param, ParamType, ToolDefinition};
use serde_json::json;

pub(super) fn definition() -> ToolDefinition {
    make_definition(
        "write_file",
        "Create or overwrite a text file in the workspace; missing parent directories are created.",
        vec![
            path_param("path", "Path of the file, relative to the workspace root."),
            Param {
                name: "content".into(),
                ty: ParamType::String {
                    min_chars: 0,
                    max_chars: LIM_W3_FILE_BYTES,
                    max_bytes: Some(LIM_W3_FILE_BYTES),
                },
                optional: false,
                description: "The whole content of the file.".into(),
            },
        ],
        EffectKind::WorkspaceWrite,
    )
}

pub(super) fn run(body: Body) -> ToolResult {
    let path = workspace_path(&body.args, "path")?;
    if path.is_root() {
        return Err(ToolError::new("is_directory"));
    }
    let content = str_arg(&body.args, "content").ok_or(ToolError::new("missing_content"))?;
    if content.len() > LIM_W3_FILE_BYTES {
        return Err(ToolError::new("too_large"));
    }
    let mut parents_created = 0u64;
    if let Some(parent) = path.parent()
        && !parent.is_root()
        && !body.dir.is_dir(parent.as_str())
    {
        let mut missing = Vec::new();
        let mut cursor = Some(parent.clone());
        while let Some(p) = cursor {
            if p.is_root() || body.dir.is_dir(p.as_str()) {
                break;
            }
            missing.push(p.clone());
            cursor = p.parent();
        }
        parents_created = missing.len() as u64;
        body.dir
            .create_dir_all(parent.as_str())
            .map_err(|e| ToolError::io("create_parent_failed", &e))?;
    }
    let created = !body.dir.exists(path.as_str());
    body.dir
        .write(path.as_str(), content.as_bytes())
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::new("no_parent"),
            _ => ToolError::io("write_failed", &e),
        })?;
    Ok((
        json!({
            "path": path.as_str(),
            "bytes_written": content.len(),
            "created": created,
            "parents_created": parents_created,
        }),
        Vec::new(),
    ))
}
