//! `[effort.<level>]` (REQ-TASK-013, REQ-CFG-007; RFC 015 with Amendment 1; external design §19.4,
//! from 0.13): the keys a level sets, what `check` refuses, and the keys the runtime is given.
//!
//! A level is an overlay of five keys. The configuration holds only what the file says; what a
//! level ships with is the core's ([`Levels::shipped`]), and a key in the file replaces the
//! shipped one ([`Config::effort_levels`]).

use super::{Config, ConfigError, range};
use mimlys_core::effort::{EffortLevel, LevelKeys, Levels};
use mimlys_core::limits::{
    LIM_C1_CONTEXT_CHARS_RANGE, LIM_C3_RECENT_STEPS_RANGE, LIM_T1_STEPS_RANGE,
    LIM_T5_DEADLINE_MS_RANGE,
};
use mimlys_core::policy::Policy;
use mimlys_core::spi::Locality;
use serde::{Deserialize, Serialize};

/// `[effort.<level>]`: each key is absent unless the level sets it, and an absent key is the
/// profile's (the backend and the context budget: the default backend's).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffortSection {
    /// A name from `[backends]`: what the level runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// LIM-T1, within the ceiling's `max_steps`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    /// LIM-T5, as `--deadline` takes it (`30s`, `15m`, `2h`, `1d`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    /// LIM-C1, at most the level's backend's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_chars: Option<u32>,
    /// LIM-C3, 1 to 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_steps: Option<u32>,
}

impl EffortSection {
    /// Whether the section sets no key.
    pub fn is_empty(&self) -> bool {
        *self == EffortSection::default()
    }

    /// The keys, with the deadline read. The text was checked when the file was loaded; one that
    /// does not read here is no key.
    fn keys(&self) -> LevelKeys {
        LevelKeys {
            backend: self.backend.clone(),
            steps: self.steps,
            deadline_ms: self
                .deadline
                .as_deref()
                .and_then(|d| super::parse_duration_ms(d).ok()),
            context_chars: self.context_chars,
            recent_steps: self.recent_steps,
        }
    }
}

impl Config {
    /// The keys of the three levels in force: what each ships with, then the file's keys over
    /// them, key by key (REQ-TASK-013).
    pub fn effort_levels(&self) -> Levels {
        let mut levels = Levels::shipped();
        for (name, section) in &self.effort {
            let Some(level) = EffortLevel::parse(name) else {
                continue;
            };
            let keys = section.keys();
            let target = match level {
                EffortLevel::Quick => &mut levels.quick,
                EffortLevel::Standard => &mut levels.standard,
                EffortLevel::Deep => &mut levels.deep,
            };
            if keys.backend.is_some() {
                target.backend = keys.backend;
            }
            if keys.steps.is_some() {
                target.steps = keys.steps;
            }
            if keys.deadline_ms.is_some() {
                target.deadline_ms = keys.deadline_ms;
            }
            if keys.context_chars.is_some() {
                target.context_chars = keys.context_chars;
            }
            if keys.recent_steps.is_some() {
                target.recent_steps = keys.recent_steps;
            }
        }
        levels
    }

    /// What `check` refuses of `[effort.*]`, each naming the level, the key and the bound
    /// (external design §19.4): a name that is not a level; a backend that is not configured; a
    /// value outside its range or above its bound; a level that names no backend where there is
    /// none to run on; levels whose backends are not all local or all on one remote endpoint.
    /// `policy` is the policy in force, whose ceiling bounds `steps`.
    pub(super) fn validate_effort(&self, policy: &Policy) -> Result<(), ConfigError> {
        for (name, section) in &self.effort {
            let Some(level) = EffortLevel::parse(name) else {
                return Err(ConfigError::new(
                    &format!("effort.{name}"),
                    crate::messages::not_an_effort_level(name),
                ));
            };
            let key = |k: &str| format!("effort.{level}.{k}");
            if let Some(steps) = section.steps {
                range(&key("steps"), steps, LIM_T1_STEPS_RANGE)?;
                if steps > policy.ceiling.max_steps {
                    return Err(ConfigError::new(
                        &key("steps"),
                        format!(
                            "{steps} is above {}, the ceiling's max_steps (policy.ceiling.max_steps)",
                            policy.ceiling.max_steps
                        ),
                    ));
                }
            }
            if let Some(text) = &section.deadline {
                let ms = super::parse_duration_ms(text)
                    .map_err(|reason| ConfigError::new(&key("deadline"), reason))?;
                if ms < LIM_T5_DEADLINE_MS_RANGE.0 || ms > LIM_T5_DEADLINE_MS_RANGE.1 {
                    return Err(ConfigError::new(
                        &key("deadline"),
                        format!(
                            "`{text}` is outside 1m..=24h (LIM-T5: {}..={} ms)",
                            LIM_T5_DEADLINE_MS_RANGE.0, LIM_T5_DEADLINE_MS_RANGE.1
                        ),
                    ));
                }
            }
            if let Some(recent) = section.recent_steps {
                range(&key("recent_steps"), recent, LIM_C3_RECENT_STEPS_RANGE)?;
            }
            // The backend the level runs on: its own, or `default`. A level that names none and
            // has nothing to run on is refused; a level that sets no key at all is no refusal
            // (`standard` ships with none, and a file without `default` loads, REQ-CFG-002).
            let backend = match &section.backend {
                Some(named) => Some(named.as_str()),
                None if self.backends.contains_key(crate::backends::DEFAULT) => {
                    Some(crate::backends::DEFAULT)
                }
                None => None,
            };
            match backend {
                None if !section.is_empty() => {
                    return Err(ConfigError::new(
                        &key("backend"),
                        format!(
                            "level `{level}` names no backend, and no backend is named `{}`, the one a level runs on without a `backend`: name one with `backend = \"<name>\"`, or write `[backends.{}]`",
                            crate::backends::DEFAULT,
                            crate::backends::DEFAULT
                        ),
                    ));
                }
                None => {}
                Some(backend) => {
                    let section_of = self.backends.get(backend).ok_or_else(|| {
                        ConfigError::new(
                            &key("backend"),
                            crate::messages::no_backend_named(backend, &self.backend_names()),
                        )
                    })?;
                    if let Some(chars) = section.context_chars {
                        range(
                            &key("context_chars"),
                            chars as usize,
                            LIM_C1_CONTEXT_CHARS_RANGE,
                        )?;
                        if chars as usize > section_of.context_chars {
                            return Err(ConfigError::new(
                                &key("context_chars"),
                                format!(
                                    "{chars} is above {}, the context_chars of backend `{backend}`, which is sized to its engine's window (backends.{backend}.context_chars)",
                                    section_of.context_chars
                                ),
                            ));
                        }
                    }
                }
            }
        }
        self.validate_effort_locality()
    }

    /// Every level's backend is local, or every level's backend has the same remote endpoint
    /// (REQ-CFG-007; RFC 015 Amendment 1, item 2): a level never changes where a task's data goes.
    /// The levels counted are the ones that can be chosen: `standard`, and each level that has
    /// a key, shipped or configured; a level that is refused when chosen sends nothing.
    fn validate_effort_locality(&self) -> Result<(), ConfigError> {
        let levels = self.effort_levels();
        struct Place<'a> {
            level: EffortLevel,
            backend: &'a str,
            locality: Locality,
            endpoint: Option<String>,
        }
        let mut places: Vec<Place<'_>> = Vec::new();
        for level in EffortLevel::ALL {
            if !levels.is_configured(level) {
                continue;
            }
            let name = levels
                .keys(level)
                .backend
                .as_deref()
                .unwrap_or(crate::backends::DEFAULT);
            // A backend that is not configured is refused above, or is the missing `default`,
            // which the commands that run steps refuse.
            let Some(section) = self.backends.get(name) else {
                continue;
            };
            places.push(Place {
                level,
                backend: name,
                locality: section.locality,
                endpoint: section
                    .url
                    .as_deref()
                    .and_then(crate::registry::endpoint_of)
                    .map(|(_, shown)| shown),
            });
        }
        let all_local = places.iter().all(|p| p.locality == Locality::Local);
        let one_remote_endpoint = places.iter().all(|p| p.locality == Locality::Remote)
            && places
                .iter()
                .all(|p| p.endpoint.is_some() && p.endpoint == places[0].endpoint);
        if all_local || one_remote_endpoint || places.is_empty() {
            return Ok(());
        }
        let describe = |p: &Place<'_>| {
            format!(
                "`{}` (backend `{}`, {}{})",
                p.level,
                p.backend,
                match p.locality {
                    Locality::Local => "local",
                    Locality::Remote => "remote",
                },
                p.endpoint
                    .as_deref()
                    .map(|e| format!(", {e}"))
                    .unwrap_or_default()
            )
        };
        // The level that departs from the first one is the one the key names.
        let first = &places[0];
        let departing = places
            .iter()
            .find(|p| (p.locality, &p.endpoint) != (first.locality, &first.endpoint))
            .unwrap_or(first);
        Err(ConfigError::new(
            &format!("effort.{}.backend", departing.level),
            format!(
                "levels {} and {} are not all local, and not all on one remote endpoint; a level never changes where a task's data goes, so every level's backend is local, or every level's backend has the same remote endpoint",
                describe(first),
                describe(departing)
            ),
        ))
    }
}

#[cfg(test)]
mod tests;
