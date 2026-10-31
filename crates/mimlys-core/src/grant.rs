//! Capability grants and the grant set (external design §9.1, §9.3;
//! REQ-CAP-001 … 007). No grant, no operation; a grant never widens; the
//! active set has a canonical hash recorded on every action and approval.

use crate::clock::Instant;
use crate::event::Actor;
use crate::hash::{Hash, hash_canonical};
use crate::ids::{GrantId, TaskId};
use crate::path::CaseMode;
use crate::policy::Ceiling;
use serde::{Deserialize, Serialize};

/// What a grant authorizes (external design §9.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantKind {
    /// A tool is visible and usable.
    Tool {
        /// The tool's name.
        name: String,
    },
    /// Reading a host directory outside the workspace.
    HostRead {
        /// Absolute host path of the directory.
        root: String,
        /// Globs relative to `root`; empty means everything under it.
        #[serde(default)]
        patterns: Vec<String>,
    },
    /// Writing a host directory (the promotion target, M3).
    HostWrite {
        /// Absolute host path of the directory.
        root: String,
        /// Globs relative to `root`; empty means everything under it.
        #[serde(default)]
        patterns: Vec<String>,
    },
    /// Network destinations (M5).
    Network {
        /// Host names; a leading `*.` allows subdomains.
        domains: Vec<String>,
        /// Allowed ports; empty means 443 only.
        #[serde(default)]
        ports: Vec<u16>,
    },
    /// A declared host tool (M6).
    HostTool {
        /// The tool's name.
        name: String,
    },
}

impl GrantKind {
    /// The kind's name, used in denials and the ceiling.
    pub fn name(&self) -> &'static str {
        match self {
            GrantKind::Tool { .. } => "tool",
            GrantKind::HostRead { .. } => "host_read",
            GrantKind::HostWrite { .. } => "host_write",
            GrantKind::Network { .. } => "network",
            GrantKind::HostTool { .. } => "host_tool",
        }
    }
}

/// The status of a grant (external design §9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantStatus {
    /// In force.
    Active,
    /// Revoked by the user or by a reduced ceiling.
    Revoked,
    /// Past `valid_until`.
    Expired,
}

/// An explicit, per-task, revocable authorization (REQ-CAP-004).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// Identifier.
    pub grant_id: GrantId,
    /// The task.
    pub task_id: TaskId,
    /// What it authorizes; never mutated (REQ-CAP-006).
    kind: GrantKind,
    /// Who issued it.
    #[serde(flatten)]
    pub issued_by: Actor,
    /// When.
    pub issued_at: Instant,
    /// Status.
    pub status: GrantStatus,
    /// Expiry, when any.
    pub valid_until: Option<Instant>,
}

impl Grant {
    /// Issues an active grant.
    pub fn issue(
        grant_id: GrantId,
        task_id: TaskId,
        kind: GrantKind,
        issued_by: Actor,
        now: Instant,
        valid_until: Option<Instant>,
    ) -> Self {
        Self {
            grant_id,
            task_id,
            kind,
            issued_by,
            issued_at: now,
            status: GrantStatus::Active,
            valid_until,
        }
    }

    /// What it authorizes.
    pub fn kind(&self) -> &GrantKind {
        &self.kind
    }

    /// Whether the grant is in force at `now`.
    pub fn is_active(&self, now: Instant) -> bool {
        self.status == GrantStatus::Active && self.valid_until.is_none_or(|until| now < until)
    }
}

/// A grant could not be changed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GrantError {
    /// No such grant.
    #[error("no such grant")]
    NotFound,
    /// Already revoked or expired.
    #[error("the grant is not active")]
    NotActive,
}

/// Read or write access to a host path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostAccess {
    /// Read.
    Read,
    /// Write.
    Write,
}

/// The grants of a task (external design §9.3; REQ-CAP-007).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GrantSet {
    grants: Vec<Grant>,
}

impl GrantSet {
    /// An empty set: only the implicit workspace rights.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a grant.
    pub fn add(&mut self, grant: Grant) {
        self.grants.push(grant);
    }

    /// Every grant, active or not.
    pub fn all(&self) -> &[Grant] {
        &self.grants
    }

    /// The grants in force at `now`.
    pub fn active(&self, now: Instant) -> impl Iterator<Item = &Grant> {
        self.grants.iter().filter(move |g| g.is_active(now))
    }

    /// Revokes a grant at once (REQ-CAP-005).
    pub fn revoke(&mut self, grant_id: GrantId) -> Result<(), GrantError> {
        let grant = self
            .grants
            .iter_mut()
            .find(|g| g.grant_id == grant_id)
            .ok_or(GrantError::NotFound)?;
        if grant.status != GrantStatus::Active {
            return Err(GrantError::NotActive);
        }
        grant.status = GrantStatus::Revoked;
        Ok(())
    }

    /// Marks grants past their `valid_until` as expired; returns their ids.
    pub fn expire(&mut self, now: Instant) -> Vec<GrantId> {
        let mut expired = Vec::new();
        for g in &mut self.grants {
            if g.status == GrantStatus::Active && g.valid_until.is_some_and(|until| now >= until) {
                g.status = GrantStatus::Expired;
                expired.push(g.grant_id);
            }
        }
        expired
    }

    /// The canonical hash of the active grant kinds, order-independent (REQ-CAP-007).
    pub fn hash(&self, now: Instant) -> Hash {
        let mut kinds: Vec<serde_json::Value> = self
            .active(now)
            .map(|g| serde_json::to_value(&g.kind).expect("a grant kind serializes"))
            .collect();
        kinds.sort_by_key(crate::hash::canonical_json);
        hash_canonical(&serde_json::Value::Array(kinds))
    }

    /// The tools visible through grants: every `tool` grant's name, and
    /// `run_host_tool` when any `host_tool` grant is active (external design
    /// §9.1: a `host_tool{name}` grant means `run_host_tool` for that tool;
    /// no `tool` grant names the gateway itself).
    pub fn visible_tools(&self, now: Instant) -> Vec<String> {
        let mut names: Vec<String> = self
            .active(now)
            .filter_map(|g| match &g.kind {
                GrantKind::Tool { name } => Some(name.clone()),
                GrantKind::HostTool { .. } => Some(crate::host_tool::RUN_HOST_TOOL.to_owned()),
                _ => None,
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Whether `host_tool` is granted.
    pub fn permits_host_tool(&self, name: &str, now: Instant) -> bool {
        self.active(now)
            .any(|g| matches!(&g.kind, GrantKind::HostTool { name: n } if n == name))
    }

    /// Whether an absolute host path may be read or written: it must lie
    /// under a granted root (lexically, both canonical) and match one of the
    /// root's patterns when any are declared.
    pub fn permits_host_path(
        &self,
        path: &str,
        access: HostAccess,
        case: CaseMode,
        now: Instant,
    ) -> bool {
        self.active(now).any(|g| match (&g.kind, access) {
            (GrantKind::HostRead { root, patterns }, HostAccess::Read)
            | (GrantKind::HostWrite { root, patterns }, HostAccess::Write) => {
                under_root(path, root, patterns, case)
            }
            _ => false,
        })
    }

    /// Whether a destination URL is covered by a `network` grant: scheme
    /// `https`, host equal to a domain or under a `*.` domain, port allowed.
    pub fn permits_destination(&self, url: &str, now: Instant) -> bool {
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        if parsed.scheme() != "https" {
            return false;
        }
        let Some(host) = parsed.host_str() else {
            return false;
        };
        let port = parsed.port().unwrap_or(443);
        let host = host.to_ascii_lowercase();
        self.active(now).any(|g| match &g.kind {
            GrantKind::Network { domains, ports } => {
                let port_ok = if ports.is_empty() {
                    port == 443
                } else {
                    ports.contains(&port)
                };
                port_ok
                    && domains
                        .iter()
                        .any(|d| domain_matches(&host, &d.to_ascii_lowercase()))
            }
            _ => false,
        })
    }

    /// Grants that the ceiling no longer permits (REQ-CAP-010); the loop revokes them.
    pub fn reconcile(&self, ceiling: &Ceiling, now: Instant) -> Vec<GrantId> {
        self.active(now)
            .filter(|g| ceiling.permits(&g.kind).is_err())
            .map(|g| g.grant_id)
            .collect()
    }
}

/// Lexical normalization of an absolute host path: one separator, no `.`
/// or `..` components, no trailing separator, case folded when asked.
fn normalize(path: &str, case: CaseMode) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split(['/', '\\']) {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let joined = format!("/{}", parts.join("/"));
    match case {
        CaseMode::Sensitive => joined,
        CaseMode::Insensitive => joined.to_lowercase(),
    }
}

fn under_root(path: &str, root: &str, patterns: &[String], case: CaseMode) -> bool {
    let p = normalize(path, case);
    let r = normalize(root, case);
    let relative = if p == r {
        ""
    } else if let Some(rest) = p.strip_prefix(&format!("{r}/")) {
        rest
    } else {
        return false;
    };
    if patterns.is_empty() {
        return true;
    }
    let mut builder = globset::GlobSetBuilder::new();
    for pattern in patterns {
        let Ok(glob) = globset::GlobBuilder::new(pattern)
            .literal_separator(true)
            .case_insensitive(case == CaseMode::Insensitive)
            .build()
        else {
            return false;
        };
        builder.add(glob);
    }
    builder
        .build()
        .map(|set| set.is_match(relative))
        .unwrap_or(false)
}

fn domain_matches(host: &str, domain: &str) -> bool {
    match domain.strip_prefix("*.") {
        Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
        None => host == domain,
    }
}

#[cfg(test)]
mod tests;
