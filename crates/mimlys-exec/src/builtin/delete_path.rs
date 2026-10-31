use super::{Body, ToolError, ToolResult, bool_arg, make_definition, path_param, workspace_path};
use mimlys_core::action::EffectKind;
use mimlys_core::tool::{Param, ParamType, ToolDefinition};
use serde_json::json;

pub(super) fn definition() -> ToolDefinition {
    make_definition(
        "delete_path",
        "Delete a file, or a directory with `recursive`. The workspace root cannot be deleted.",
        vec![
            path_param("path", "Path to delete, relative to the workspace root."),
            Param {
                name: "recursive".into(),
                ty: ParamType::Boolean,
                optional: true,
                description: "Required to delete a non-empty directory.".into(),
            },
        ],
        EffectKind::WorkspaceWrite,
    )
}

pub(super) fn run(body: Body) -> ToolResult {
    let path = workspace_path(&body.args, "path")?;
    if path.is_root() {
        return Err(ToolError::new("is_root"));
    }
    let recursive = bool_arg(&body.args, "recursive", false);
    let meta = body
        .dir
        .symlink_metadata(path.as_str())
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::new("not_found"),
            _ => ToolError::io("stat_failed", &e),
        })?;
    if meta.is_dir() {
        let count = count_entries(&body.dir, path.as_str())?;
        if count > 0 && !recursive {
            return Err(ToolError {
                detail: "not_empty",
                payload: json!({ "entries": count }),
            });
        }
        if recursive {
            body.dir
                .remove_dir_all(path.as_str())
                .map_err(|e| ToolError::io("delete_failed", &e))?;
        } else {
            body.dir
                .remove_dir(path.as_str())
                .map_err(|e| ToolError::io("delete_failed", &e))?;
        }
        Ok((
            json!({ "path": path.as_str(), "deleted": count + 1 }),
            Vec::new(),
        ))
    } else {
        body.dir
            .remove_file(path.as_str())
            .map_err(|e| ToolError::io("delete_failed", &e))?;
        Ok((json!({ "path": path.as_str(), "deleted": 1 }), Vec::new()))
    }
}

fn count_entries(dir: &cap_std::fs::Dir, rel: &str) -> Result<u64, ToolError> {
    let mut count = 0;
    for entry in dir
        .read_dir(rel)
        .map_err(|e| ToolError::io("read_dir_failed", &e))?
    {
        let entry = entry.map_err(|e| ToolError::io("read_dir_failed", &e))?;
        count += 1;
        if entry
            .file_type()
            .map_err(|e| ToolError::io("stat_failed", &e))?
            .is_dir()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            count += count_entries(dir, &format!("{rel}/{name}"))?;
        }
    }
    Ok(count)
}
