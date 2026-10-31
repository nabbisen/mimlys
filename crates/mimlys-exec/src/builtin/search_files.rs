use super::{Body, ToolError, ToolResult, check_cancel, int_arg, make_definition, str_arg};
use cap_std::fs::Dir;
use mimlys_core::action::EffectKind;
use mimlys_core::limits::{LIM_I7_ENTRIES, LIM_W3_FILE_BYTES};
use mimlys_core::observation::{Notice, SkipReason};
use mimlys_core::tool::{Param, ParamType, ToolDefinition};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Characters of a matching line shown to the model; longer lines carry `truncated: true`.
const MATCH_LINE_CHARS: usize = 400;

pub(super) fn definition() -> ToolDefinition {
    make_definition(
        "search_files",
        "Search workspace files line by line with a regular expression; `glob` restricts the files (for example `**/*.rs`). Symbolic links and files over the size limit are skipped and counted.",
        vec![
            Param {
                name: "pattern".into(),
                ty: ParamType::String {
                    min_chars: 1,
                    max_chars: 1000,
                    max_bytes: None,
                },
                optional: false,
                description: "Regular expression.".into(),
            },
            Param {
                name: "glob".into(),
                ty: ParamType::String {
                    min_chars: 1,
                    max_chars: 500,
                    max_bytes: None,
                },
                optional: true,
                description: "Glob over relative paths.".into(),
            },
            Param {
                name: "max_results".into(),
                ty: ParamType::Integer {
                    min: 1,
                    max: LIM_I7_ENTRIES as i64,
                },
                optional: true,
                description: "Maximum matches to return.".into(),
            },
        ],
        EffectKind::WorkspaceRead,
    )
}

struct Search<'a> {
    regex: regex::Regex,
    glob: Option<globset::GlobMatcher>,
    max_results: usize,
    matches: Vec<Value>,
    total: u64,
    skipped: BTreeMap<SkipReason, u64>,
    cancel: &'a mimlys_core::spi::Cancellation,
}

pub(super) fn run(body: Body) -> ToolResult {
    let pattern = str_arg(&body.args, "pattern").ok_or(ToolError::new("missing_pattern"))?;
    let regex = regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|_| ToolError::new("invalid_pattern"))?;
    let glob = match str_arg(&body.args, "glob") {
        Some(g) => Some(
            globset::GlobBuilder::new(g)
                .literal_separator(true)
                .build()
                .map_err(|_| ToolError::new("invalid_glob"))?
                .compile_matcher(),
        ),
        None => None,
    };
    let max_results = int_arg(&body.args, "max_results", LIM_I7_ENTRIES as i64)
        .clamp(1, LIM_I7_ENTRIES as i64) as usize;
    let mut search = Search {
        regex,
        glob,
        max_results,
        matches: Vec::new(),
        total: 0,
        skipped: BTreeMap::new(),
        cancel: &body.cancel,
    };
    search.walk(&body.dir, "")?;
    let mut notices: Vec<Notice> = search
        .skipped
        .iter()
        .map(|(reason, count)| Notice::EntriesSkipped {
            reason: *reason,
            count: *count,
        })
        .collect();
    let truncated = search.total as usize > max_results;
    if truncated {
        notices.push(Notice::EntriesTruncated {
            total: search.total,
        });
    }
    Ok((
        json!({ "matches": search.matches, "truncated": truncated }),
        notices,
    ))
}

impl Search<'_> {
    fn walk(&mut self, dir: &Dir, rel: &str) -> Result<(), ToolError> {
        check_cancel(self.cancel)?;
        let read = if rel.is_empty() {
            dir.read_dir(".")
        } else {
            dir.read_dir(rel)
        };
        let mut entries: Vec<_> = read
            .map_err(|e| ToolError::io("read_dir_failed", &e))?
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let file_type = entry
                .file_type()
                .map_err(|e| ToolError::io("stat_failed", &e))?;
            if file_type.is_symlink() {
                *self.skipped.entry(SkipReason::Symlink).or_default() += 1;
                continue;
            }
            if file_type.is_dir() {
                self.walk(dir, &child)?;
                continue;
            }
            if self.glob.as_ref().is_some_and(|g| !g.is_match(&child)) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if size as usize > LIM_W3_FILE_BYTES {
                *self.skipped.entry(SkipReason::TooLarge).or_default() += 1;
                continue;
            }
            let Ok(bytes) = dir.read(&child) else {
                *self.skipped.entry(SkipReason::Unreadable).or_default() += 1;
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            for (i, line) in text.lines().enumerate() {
                if self.regex.is_match(line) {
                    self.total += 1;
                    if self.matches.len() < self.max_results {
                        let shown: String = line.chars().take(MATCH_LINE_CHARS).collect();
                        let mut m = json!({ "path": child, "line": i + 1, "text": shown });
                        if line.chars().count() > MATCH_LINE_CHARS {
                            m["truncated"] = Value::Bool(true);
                        }
                        self.matches.push(m);
                    }
                }
            }
            check_cancel(self.cancel)?;
        }
        Ok(())
    }
}
