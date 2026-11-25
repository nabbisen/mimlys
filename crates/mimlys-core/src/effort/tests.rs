//! The level's rules as pure functions (REQ-TASK-013; external design §19.4): which level a task
//! gets, whether it can be chosen, what bounds it, and what it runs with.

use super::*;

fn no_bound() -> (EffortLevel, Bound) {
    (EffortLevel::Deep, Bound::None)
}

/// A level is one of three fixed identifiers, ordered, and read exactly as written.
#[test]
fn the_levels_are_three_fixed_identifiers_in_order() {
    assert!(EffortLevel::Quick < EffortLevel::Standard);
    assert!(EffortLevel::Standard < EffortLevel::Deep);
    for level in EffortLevel::ALL {
        assert_eq!(EffortLevel::parse(level.name()), Some(level));
        assert_eq!(
            serde_json::to_value(level).unwrap(),
            serde_json::json!(level.name())
        );
    }
    for other in ["", "Quick", "fast", "deep ", "max"] {
        assert_eq!(EffortLevel::parse(other), None, "{other:?}");
    }
}

/// REQ-TASK-013: `quick` ships its two keys; `standard` and `deep` ship none, so `deep` cannot be
/// chosen until a key is configured, and `standard` always can.
#[test]
fn shipped_levels() {
    let shipped = Levels::shipped();
    assert_eq!(shipped.quick.steps, Some(22));
    assert_eq!(shipped.quick.deadline_ms, Some(600_000));
    assert_eq!(
        LevelKeys {
            steps: Some(22),
            deadline_ms: Some(600_000),
            ..LevelKeys::default()
        },
        shipped.quick,
        "quick ships steps and deadline and nothing else"
    );
    assert!(shipped.is_configured(EffortLevel::Quick));
    assert!(shipped.is_configured(EffortLevel::Standard));
    assert!(!shipped.is_configured(EffortLevel::Deep));
}

/// §19.4, "Choosing": without a flag, `standard`; the flag is the user's; a fork takes its source's.
#[test]
fn choosing_a_level() {
    let levels = Levels {
        deep: LevelKeys {
            backend: Some("big".into()),
            ..LevelKeys::default()
        },
        ..Levels::shipped()
    };
    let bound = no_bound();
    assert_eq!(
        choose(None, None, &bound, &levels),
        Ok((EffortLevel::Standard, ChosenBy::Default))
    );
    assert_eq!(
        choose(Some(EffortLevel::Quick), None, &bound, &levels),
        Ok((EffortLevel::Quick, ChosenBy::User))
    );
    assert_eq!(
        choose(Some(EffortLevel::Standard), None, &bound, &levels),
        Ok((EffortLevel::Standard, ChosenBy::User))
    );
    // A fork takes its source's level unless the flag is given.
    assert_eq!(
        choose(None, Some(EffortLevel::Deep), &bound, &levels),
        Ok((EffortLevel::Deep, ChosenBy::Fork))
    );
    assert_eq!(
        choose(
            Some(EffortLevel::Quick),
            Some(EffortLevel::Deep),
            &bound,
            &levels
        ),
        Ok((EffortLevel::Quick, ChosenBy::User))
    );
}

/// REQ-TASK-013: a level with no key, shipped or configured, is refused when chosen, whoever chose
/// it, naming the section that configures it.
#[test]
fn a_level_without_keys_is_refused() {
    let levels = Levels::shipped();
    let refusal = choose(Some(EffortLevel::Deep), None, &no_bound(), &levels).unwrap_err();
    assert_eq!(
        refusal,
        Refusal::NoKeys {
            level: EffortLevel::Deep
        }
    );
    assert!(refusal.to_string().contains("[effort.deep]"), "{refusal}");
    // A fork of a `deep` task, when `deep` is no longer configured, is refused the same way.
    assert_eq!(
        choose(None, Some(EffortLevel::Deep), &no_bound(), &levels),
        Err(refusal)
    );
}

/// §19.4, "`effort_max`": the bound is the lower of the profile's and the ceiling's; a level above
/// it is refused naming what sets it; without a flag the level is `standard` or the bound when it
/// is lower.
#[test]
fn effort_max_bounds_the_level() {
    let levels = Levels::shipped();
    // The lower of the two, and which one it is.
    assert_eq!(
        bound_of(None, "workspace", None),
        (EffortLevel::Deep, Bound::None)
    );
    assert_eq!(
        bound_of(Some(EffortLevel::Standard), "workspace", None),
        (EffortLevel::Standard, Bound::Ceiling)
    );
    assert_eq!(
        bound_of(None, "workspace", Some(EffortLevel::Quick)),
        (EffortLevel::Quick, Bound::Profile("workspace".into()))
    );
    assert_eq!(
        bound_of(
            Some(EffortLevel::Standard),
            "workspace",
            Some(EffortLevel::Quick)
        ),
        (EffortLevel::Quick, Bound::Profile("workspace".into()))
    );
    assert_eq!(
        bound_of(
            Some(EffortLevel::Quick),
            "workspace",
            Some(EffortLevel::Standard)
        ),
        (EffortLevel::Quick, Bound::Ceiling),
        "a profile above the ceiling is bounded by the ceiling"
    );

    let by_profile = bound_of(None, "read_only", Some(EffortLevel::Standard));
    let refusal = choose(Some(EffortLevel::Deep), None, &by_profile, &levels).unwrap_err();
    assert_eq!(
        refusal,
        Refusal::AboveMax {
            level: EffortLevel::Deep,
            max: EffortLevel::Standard,
            bound: Bound::Profile("read_only".into()),
        }
    );
    assert!(
        refusal
            .to_string()
            .contains("[policy.profiles.read_only] effort_max"),
        "{refusal}"
    );
    let by_ceiling = bound_of(Some(EffortLevel::Standard), "workspace", None);
    let refusal = choose(Some(EffortLevel::Deep), None, &by_ceiling, &levels).unwrap_err();
    assert!(
        refusal.to_string().contains("[policy.ceiling] effort_max"),
        "{refusal}"
    );

    // Without the flag: `standard`, or the bound when it is lower, chosen by default.
    let at_quick = bound_of(Some(EffortLevel::Quick), "workspace", None);
    assert_eq!(
        choose(None, None, &at_quick, &levels),
        Ok((EffortLevel::Quick, ChosenBy::Default))
    );
    assert_eq!(
        choose(None, None, &by_ceiling, &levels),
        Ok((EffortLevel::Standard, ChosenBy::Default))
    );
    // A fork whose source is above the bound now is refused, not lowered silently.
    assert!(matches!(
        choose(None, Some(EffortLevel::Standard), &at_quick, &levels),
        Err(Refusal::AboveMax { .. })
    ));
}

/// §19.4, "The envelope": a flag wins over the level, the level over the profile, and the profile
/// over the register's default; the context budget is the level's, else its backend's.
#[test]
fn the_envelope_is_resolved_by_precedence() {
    let backend_context = |name: &str| if name == "big" { 24_000 } else { 32_000 };
    let none = LevelKeys::default();
    let level = LevelKeys {
        backend: Some("big".into()),
        steps: Some(30),
        deadline_ms: Some(3_600_000),
        context_chars: Some(20_000),
        recent_steps: Some(3),
    };
    fn base(
        keys: &LevelKeys,
        flag_steps: Option<u32>,
        flag_deadline_ms: Option<i64>,
    ) -> Inputs<'_> {
        Inputs {
            keys,
            flag_steps,
            flag_deadline_ms,
            profile_steps: 40,
            default_backend: "default",
        }
    }

    // Nothing but the profile: today's behaviour, every key resolved.
    assert_eq!(
        resolve(&base(&none, None, None), backend_context),
        Envelope {
            backend: "default".into(),
            steps: 40,
            deadline_ms: LIM_T5_DEADLINE_MS,
            context_chars: Some(32_000),
            recent_steps: 6,
        }
    );
    // The level over the profile and the register's default.
    assert_eq!(
        resolve(&base(&level, None, None), backend_context),
        Envelope {
            backend: "big".into(),
            steps: 30,
            deadline_ms: 3_600_000,
            context_chars: Some(20_000),
            recent_steps: 3,
        }
    );
    // The flag over the level.
    let flagged = resolve(&base(&level, Some(12), Some(120_000)), backend_context);
    assert_eq!(flagged.steps, 12);
    assert_eq!(flagged.deadline_ms, 120_000);
    assert_eq!(flagged.backend, "big", "a flag changes only what it names");
    // A level that names a backend and not a context budget takes its backend's.
    let named = LevelKeys {
        backend: Some("big".into()),
        ..LevelKeys::default()
    };
    assert_eq!(
        resolve(&base(&named, None, None), backend_context).context_chars,
        Some(24_000)
    );
}

/// The record's words: a level, who chose it and the envelope, with no field added that is empty.
#[test]
fn the_record_is_what_the_design_says() {
    let effort = Effort {
        level: EffortLevel::Quick,
        chosen_by: ChosenBy::User,
        envelope: Envelope {
            backend: "default".into(),
            steps: 22,
            deadline_ms: 600_000,
            context_chars: Some(32_000),
            recent_steps: 6,
        },
    };
    assert_eq!(
        serde_json::to_value(&effort).unwrap(),
        serde_json::json!({
            "level": "quick",
            "chosen_by": "user",
            "envelope": {
                "backend": "default",
                "steps": 22,
                "deadline_ms": 600_000,
                "context_chars": 32_000,
                "recent_steps": 6,
            }
        })
    );
    let back: Effort = serde_json::from_value(serde_json::to_value(&effort).unwrap()).unwrap();
    assert_eq!(back, effort);
}
