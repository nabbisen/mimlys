use super::{
    Body, ToolError, ToolResult, check_cancel, int_arg, make_definition, path_param, workspace_path,
};
use cap_std::fs::Dir;
use mimlys_core::action::EffectKind;
use mimlys_core::limits::LIM_I7_ENTRIES;
use mimlys_core::observation::Notice;
use mimlys_core::spi::Cancellation;
use mimlys_core::tool::{Param, ParamType, ToolDefinition};
use serde_json::{Value, json};

pub(super) fn definition() -> ToolDefinition {
    make_definition(
        "list_dir",
        "List entries of a workspace directory, up to `depth` levels deep (1 to 3). Entries are files, directories or symlinks.",
        vec![
            path_param("path", "Directory to list; \".\" is the workspace root."),
            Param {
                name: "depth".into(),
                ty: ParamType::Integer { min: 1, max: 3 },
                optional: true,
                description: "Levels to descend.".into(),
            },
        ],
        EffectKind::WorkspaceRead,
    )
}

pub(super) fn run(body: Body) -> ToolResult {
    let path = workspace_path(&body.args, "path")?;
    let depth = int_arg(&body.args, "depth", 1).clamp(1, 3) as usize;
    if !path.is_root() && !body.dir.is_dir(path.as_str()) {
        return Err(if body.dir.exists(path.as_str()) {
            ToolError::new("not_a_directory")
        } else {
            ToolError::new("not_found")
        });
    }
    let mut listing = Listing {
        entries: Vec::new(),
        total: 0,
        cancel: &body.cancel,
    };
    listing.walk(&body.dir, path.as_str(), depth)?;
    let truncated = listing.total as usize > LIM_I7_ENTRIES;
    let notices = if truncated {
        vec![Notice::EntriesTruncated {
            total: listing.total,
        }]
    } else {
        Vec::new()
    };
    Ok((
        json!({ "path": path.as_str(), "entries": listing.entries, "truncated": truncated }),
        notices,
    ))
}

struct Listing<'a> {
    entries: Vec<Value>,
    total: u64,
    cancel: &'a Cancellation,
}

impl Listing<'_> {
    /// Lists `rel` sorted by name; symbolic links are reported as `symlink`
    /// without their target and are never descended.
    fn walk(&mut self, dir: &Dir, rel: &str, depth: usize) -> Result<(), ToolError> {
        check_cancel(self.cancel)?;
        let read = if rel.is_empty() {
            dir.read_dir(".")
        } else {
            dir.read_dir(rel)
        };
        let mut names: Vec<_> = read
            .map_err(|e| ToolError::io("read_dir_failed", &e))?
            .filter_map(Result::ok)
            .collect();
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let file_type = entry
                .file_type()
                .map_err(|e| ToolError::io("stat_failed", &e))?;
            self.total += 1;
            if self.entries.len() < LIM_I7_ENTRIES {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let kind = if file_type.is_symlink() {
                    "symlink"
                } else if file_type.is_dir() {
                    "dir"
                } else {
                    "file"
                };
                self.entries
                    .push(json!({ "path": child, "type": kind, "size": size }));
            }
            if file_type.is_dir() && !file_type.is_symlink() && depth > 1 {
                self.walk(dir, &child, depth - 1)?;
            }
        }
        Ok(())
    }
}
