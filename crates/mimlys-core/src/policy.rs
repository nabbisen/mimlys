//! The administrator's policy: ceiling, profiles, exclusions and recovery
//! options (external design §9.2, §19.1; REQ-CAP-003, REQ-CAP-009,
//! REQ-CAP-010, REQ-CFG-004). The policy file deserializes into these types
//! (handoff 010); the loop and the hooks read them.

use crate::grant::GrantKind;
use crate::hash::{Hash, hash_canonical};
use crate::limits::{
    LIM_T1_STEPS_RANGE, LIM_T2_CONSECUTIVE_REJECTIONS_RANGE, LIM_T3_EXECUTIONS_PER_ACTION_RANGE,
    LIM_T4_IDENTICAL_DECISIONS_RANGE, LIM_T6_CONSECUTIVE_FAILURES_RANGE,
    LIM_W4_MATERIALIZE_EXCLUSIONS, LIM_W5_SECRET_PATTERNS,
};
use crate::path::{CaseMode, WorkspacePath};
use crate::rejection::DeniedReason;
use crate::step::Budgets;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a profile does when the user rejects an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnReject {
    /// The rejection becomes feedback and the task continues.
    #[default]
    Continue,
    /// The task halts.
    Halt,
}

/// What a profile does with a running write action when the task is cancelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnCancel {
    /// Restore the pre-execution snapshot.
    #[default]
    Rollback,
    /// Keep the partial state and say so.
    Keep,
}

/// The administrator's bound on what a task may be granted (REQ-CAP-003).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ceiling {
    /// Roots a `host_read` grant may name; a granted root must be one of these or below it.
    #[serde(default)]
    pub host_read_roots: Vec<String>,
    /// Roots a `host_write` grant may name.
    #[serde(default)]
    pub host_write_roots: Vec<String>,
    /// Domains a `network` grant may name (exact, or `*.` prefixed).
    #[serde(default)]
    pub network_domains: Vec<String>,
    /// Tools a `tool` grant may name.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Host tools a `host_tool` grant may name.
    #[serde(default)]
    pub host_tools: Vec<String>,
    /// Highest step budget any profile may set (LIM-T1 range).
    pub max_steps: u32,
}

/// A refusal by the ceiling.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the ceiling does not permit this {grant_kind} grant: {reason:?}")]
pub struct CeilingDenied {
    /// Why.
    pub reason: DeniedReason,
    /// The grant kind.
    pub grant_kind: &'static str,
    /// What the ceiling permits, for the error message (CLR-3).
    pub permitted: Vec<String>,
}

impl Ceiling {
    /// Whether a grant of this kind and scope is within the ceiling.
    pub fn permits(&self, kind: &GrantKind) -> Result<(), CeilingDenied> {
        let (ok, permitted) = match kind {
            GrantKind::Tool { name } => (self.tools.iter().any(|t| t == name), self.tools.clone()),
            GrantKind::HostRead { root, .. } => (
                self.host_read_roots.iter().any(|r| root_within(root, r)),
                self.host_read_roots.clone(),
            ),
            GrantKind::HostWrite { root, .. } => (
                self.host_write_roots.iter().any(|r| root_within(root, r)),
                self.host_write_roots.clone(),
            ),
            GrantKind::Network { domains, .. } => (
                domains
                    .iter()
                    .all(|d| self.network_domains.iter().any(|c| domain_within(d, c))),
                self.network_domains.clone(),
            ),
            GrantKind::HostTool { name } => (
                self.host_tools.iter().any(|t| t == name),
                self.host_tools.clone(),
            ),
        };
        if ok {
            Ok(())
        } else {
            Err(CeilingDenied {
                reason: DeniedReason::GrantMissing,
                grant_kind: kind.name(),
                permitted,
            })
        }
    }
}

fn root_within(root: &str, ceiling_root: &str) -> bool {
    let r = root.replace('\\', "/");
    let c = ceiling_root.replace('\\', "/");
    let c = c.trim_end_matches('/');
    r == c || r.starts_with(&format!("{c}/"))
}

fn domain_within(domain: &str, ceiling_domain: &str) -> bool {
    match ceiling_domain.strip_prefix("*.") {
        Some(suffix) => {
            domain == suffix || domain.ends_with(&format!(".{suffix}")) || domain == ceiling_domain
        }
        None => domain == ceiling_domain,
    }
}

/// A named set of defaults within the ceiling (external design §9.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Name.
    pub name: String,
    /// Tools granted at creation.
    pub tools: Vec<String>,
    /// Declared host tools granted at creation (`host_tool` grants; RFC 011 §6).
    #[serde(default)]
    pub host_tools: Vec<String>,
    /// Budgets.
    #[serde(default)]
    pub budgets: Budgets,
    /// Tools that always need approval in this profile.
    #[serde(default)]
    pub approval_tools: Vec<String>,
    /// Whether promotion is available (the `project` profile).
    #[serde(default)]
    pub promote: bool,
    /// Whether a task may communicate outside the host: a `remote` backend
    /// now, tools with external effects later (REQ-SEC-010). One meaning,
    /// one key; false in every shipped profile.
    #[serde(default)]
    pub external: bool,
    /// On rejection.
    #[serde(default)]
    pub on_reject: OnReject,
    /// On cancellation of a running write action.
    #[serde(default)]
    pub on_cancel: OnCancel,
}

impl Profile {
    /// Whether the profile lists `tool` for approval (REQ-APR-001).
    pub fn requires_approval(&self, tool: &str) -> bool {
        self.approval_tools.iter().any(|t| t == tool)
    }
}

/// Patterns excluded from materialization, reading and promotion (REQ-WS-003).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exclusions {
    /// LIM-W4.
    pub materialize: Vec<String>,
    /// LIM-W5.
    pub secrets: Vec<String>,
}

/// Recovery options (REQ-PERS-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    /// Return a cleanly restored task to `running` instead of halting it.
    #[serde(default)]
    pub auto_resume: bool,
}

/// The policy (external design §19.1, REQ-CFG-004).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Schema version of the policy (REQ-CAP-010).
    pub config_version: u32,
    /// Profile used when a task names none.
    pub default_profile: String,
    /// Whether a `remote` backend may be used (REQ-SEC-010).
    #[serde(default)]
    pub remote_backend_allowed: bool,
    /// The ceiling.
    pub ceiling: Ceiling,
    /// Profiles by name.
    pub profiles: BTreeMap<String, Profile>,
    /// Exclusions.
    pub exclusions: Exclusions,
    /// Mandatory clauses whose best-effort enforcement the administrator accepts (RFC 011 §5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accept_best_effort: Vec<crate::confine::Clause>,
    /// Declared host tools by name (RFC 011 §6); a declaration admits the name to `ceiling.host_tools`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub host_tools: BTreeMap<String, crate::host_tool::HostToolDeclaration>,
    /// Recovery options.
    #[serde(default)]
    pub recovery: Recovery,
}

/// A policy is invalid; the key path names the offending value and the
/// message states the permitted range or ceiling (CLR-3).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("policy.{key}: {reason}")]
pub struct PolicyError {
    /// Key path.
    pub key: String,
    /// What is wrong, with the permitted values.
    pub reason: String,
}

/// The built-in tools known at this milestone, used by the shipped profiles.
pub const BUILTIN_TOOLS: [&str; 8] = [
    "read_file",
    "write_file",
    "replace_in_file",
    "list_dir",
    "search_files",
    "delete_path",
    "run_script",
    "run_wasm",
];

impl Policy {
    /// The shipped policy with the three profiles of external design §9.2.
    pub fn shipped() -> Self {
        let all: Vec<String> = BUILTIN_TOOLS.iter().map(|s| (*s).to_owned()).collect();
        let read_only = Profile {
            name: "read_only".into(),
            tools: vec!["read_file".into(), "list_dir".into(), "search_files".into()],
            host_tools: vec![],
            budgets: Budgets::default(),
            approval_tools: vec![],
            promote: false,
            external: false,
            on_reject: OnReject::Continue,
            on_cancel: OnCancel::Rollback,
        };
        let workspace = Profile {
            name: "workspace".into(),
            tools: all.clone(),
            ..read_only.clone()
        };
        let project = Profile {
            name: "project".into(),
            promote: true,
            ..workspace.clone()
        };
        let mut profiles = BTreeMap::new();
        for p in [read_only, workspace, project] {
            profiles.insert(p.name.clone(), p);
        }
        Self {
            config_version: 1,
            default_profile: "workspace".into(),
            remote_backend_allowed: false,
            accept_best_effort: vec![],
            host_tools: BTreeMap::new(),
            ceiling: Ceiling {
                host_read_roots: vec![],
                host_write_roots: vec![],
                network_domains: vec![],
                // `run_host_tool` is never a `tool` grant: a `host_tool` grant makes it visible (§9.1).
                tools: all.clone(),
                host_tools: vec![],
                max_steps: LIM_T1_STEPS_RANGE.1.min(200),
            },
            profiles,
            exclusions: Exclusions {
                materialize: LIM_W4_MATERIALIZE_EXCLUSIONS
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                secrets: LIM_W5_SECRET_PATTERNS
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
            },
            recovery: Recovery::default(),
        }
    }

    /// The profile by name.
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }

    /// Validates the policy against the ceiling and the limits register (REQ-CAP-003, REQ-CFG-004).
    pub fn validate(&self) -> Result<(), PolicyError> {
        if self.config_version != 1 {
            return Err(PolicyError {
                key: "config_version".into(),
                reason: "must be 1".into(),
            });
        }
        if !self.profiles.contains_key(&self.default_profile) {
            return Err(PolicyError {
                key: "default_profile".into(),
                reason: format!(
                    "must name a profile; profiles: {:?}",
                    self.profiles.keys().collect::<Vec<_>>()
                ),
            });
        }
        in_range(
            "ceiling.max_steps",
            i64::from(self.ceiling.max_steps),
            LIM_T1_STEPS_RANGE,
        )?;
        if self
            .ceiling
            .tools
            .iter()
            .any(|t| t.as_str() == crate::host_tool::RUN_HOST_TOOL)
        {
            return Err(PolicyError {
                key: "ceiling.tools".into(),
                reason: crate::host_tool::RUN_HOST_TOOL_NOT_A_GRANT.into(),
            });
        }
        for (name, p) in &self.profiles {
            if name != &p.name {
                return Err(PolicyError {
                    key: format!("profiles.{name}.name"),
                    reason: format!("must equal the key `{name}`"),
                });
            }
            if let Some(tool) = p
                .tools
                .iter()
                .find(|t| t.as_str() == crate::host_tool::RUN_HOST_TOOL)
            {
                return Err(PolicyError {
                    key: format!("profiles.{name}.tools"),
                    reason: format!("`{tool}`: {}", crate::host_tool::RUN_HOST_TOOL_NOT_A_GRANT),
                });
            }
            for tool in &p.host_tools {
                if !self.ceiling.host_tools.contains(tool) {
                    return Err(PolicyError {
                        key: format!("profiles.{name}.host_tools"),
                        reason: format!(
                            "`{tool}` is not a declared host tool; declared: {:?}",
                            self.ceiling.host_tools
                        ),
                    });
                }
            }
            // `approval_tools` may name the gateway: approval then covers every host tool.
            for tool in p.tools.iter().chain(
                p.approval_tools
                    .iter()
                    .filter(|t| t.as_str() != crate::host_tool::RUN_HOST_TOOL),
            ) {
                if !self.ceiling.tools.contains(tool) {
                    return Err(PolicyError {
                        key: format!("profiles.{name}.tools"),
                        reason: format!(
                            "`{tool}` is not in ceiling.tools {:?}",
                            self.ceiling.tools
                        ),
                    });
                }
            }
            let b = &p.budgets;
            let key = |f: &str| format!("profiles.{name}.budgets.{f}");
            in_range(
                &key("steps"),
                i64::from(b.steps),
                (LIM_T1_STEPS_RANGE.0, self.ceiling.max_steps),
            )?;
            in_range(
                &key("consecutive_rejections"),
                i64::from(b.consecutive_rejections),
                LIM_T2_CONSECUTIVE_REJECTIONS_RANGE,
            )?;
            in_range(
                &key("consecutive_failures"),
                i64::from(b.consecutive_failures),
                LIM_T6_CONSECUTIVE_FAILURES_RANGE,
            )?;
            in_range(
                &key("identical_decisions"),
                i64::from(b.identical_decisions),
                LIM_T4_IDENTICAL_DECISIONS_RANGE,
            )?;
            in_range(
                &key("executions_per_action"),
                i64::from(b.executions_per_action),
                LIM_T3_EXECUTIONS_PER_ACTION_RANGE,
            )?;
        }
        for (key, patterns) in [
            ("exclusions.materialize", &self.exclusions.materialize),
            ("exclusions.secrets", &self.exclusions.secrets),
        ] {
            ExclusionMatcher::new(patterns, CaseMode::Sensitive).map_err(|p| PolicyError {
                key: key.into(),
                reason: format!("pattern `{p}` does not compile"),
            })?;
        }
        Ok(())
    }

    /// The canonical hash of the policy (REQ-CAP-010).
    pub fn hash(&self) -> Hash {
        hash_canonical(&serde_json::to_value(self).expect("a policy serializes"))
    }

    /// Grants a new task receives from its profile: one `tool` grant per listed tool.
    pub fn initial_grants(&self, profile: &Profile) -> Vec<GrantKind> {
        profile
            .tools
            .iter()
            .map(|t| GrantKind::Tool { name: t.clone() })
            .chain(
                profile
                    .host_tools
                    .iter()
                    .map(|t| GrantKind::HostTool { name: t.clone() }),
            )
            .collect()
    }
}

fn in_range(
    key: &str,
    value: i64,
    range: (impl Into<i64>, impl Into<i64>),
) -> Result<(), PolicyError> {
    let (min, max) = (range.0.into(), range.1.into());
    if value < min || value > max {
        return Err(PolicyError {
            key: key.to_owned(),
            reason: format!("{value} is outside {min}..={max}"),
        });
    }
    Ok(())
}

/// Matches workspace paths against exclusion patterns (REQ-WS-003):
/// `name/` excludes a directory and everything below it at any depth;
/// `*.ext` and plain names match at any depth.
#[derive(Debug, Clone)]
pub struct ExclusionMatcher {
    set: globset::GlobSet,
}

impl ExclusionMatcher {
    /// Compiles the patterns; returns the first pattern that does not compile.
    pub fn new(patterns: &[String], case: CaseMode) -> Result<Self, String> {
        let mut builder = globset::GlobSetBuilder::new();
        for pattern in patterns {
            let forms: Vec<String> = match pattern.strip_suffix('/') {
                Some(dir) => vec![
                    format!("**/{dir}"),
                    format!("**/{dir}/**"),
                    dir.to_owned(),
                    format!("{dir}/**"),
                ],
                None => vec![format!("**/{pattern}"), pattern.clone()],
            };
            for form in forms {
                let glob = globset::GlobBuilder::new(&form)
                    .literal_separator(true)
                    .case_insensitive(case == CaseMode::Insensitive)
                    .build()
                    .map_err(|_| pattern.clone())?;
                builder.add(glob);
            }
        }
        let set = builder.build().map_err(|_| patterns.join(","))?;
        Ok(Self { set })
    }

    /// Whether the path, or any of its ancestors, is excluded.
    pub fn is_excluded(&self, path: &WorkspacePath) -> bool {
        self.set.is_match(path.as_str())
    }
}

#[cfg(test)]
mod tests;
