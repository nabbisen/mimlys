//! `[effort.<level>]` and `effort_max` (REQ-TASK-013, REQ-CFG-007): what `check` refuses, each
//! case naming the level, the key and the bound; the keys a level ships with and what a file
//! replaces; a configuration of 0.12 that loads unchanged.

use super::*;
use mimlys_core::effort::EffortLevel;

/// A version-2 file: two local backends (`default` and `big`, the second one on another port),
/// then `extra`.
fn file(extra: &str) -> String {
    format!(
        "config_version = 2\n\
         [backends.default]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:8080/v1\"\nmodel = \"small\"\ncontext_chars = 16000\n\
         [backends.big]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:8081/v1\"\nmodel = \"large\"\ncontext_chars = 24000\n\
         {extra}"
    )
}

/// What loading `text` is refused with: the key and the reason.
fn refused(text: &str) -> (String, String) {
    let error = Config::parse(text)
        .and_then(|config| config.validate().map(|()| config))
        .expect_err(&format!("{text} should be refused"));
    (error.key, error.reason)
}

/// REQ-TASK-013: each refusal names the level, the key and the bound (external design §19.4).
#[test]
fn check_refuses_each_case_naming_the_level_the_key_and_the_bound() {
    // (the file's tail, the key named, the words the reason holds)
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "[effort.fast]\nsteps = 5\n",
            "effort.fast",
            &[
                "`fast` is not an effort level",
                "`quick`, `standard` and `deep`",
            ],
        ),
        (
            "[effort.deep]\nbackend = \"huge\"\n",
            "effort.deep.backend",
            &["`huge`", "`default`", "`big`"],
        ),
        (
            "[effort.deep]\nbackend = \"big\"\ncontext_chars = 24001\n",
            "effort.deep.context_chars",
            &["24001", "24000", "`big`"],
        ),
        (
            "[effort.quick]\ncontext_chars = 16001\n",
            "effort.quick.context_chars",
            &["16001", "16000", "`default`"],
        ),
        (
            "[policy.ceiling]\nmax_steps = 50\n[effort.deep]\nbackend = \"big\"\nsteps = 51\n",
            "effort.deep.steps",
            &["51", "50", "max_steps"],
        ),
        (
            "[effort.quick]\nsteps = 0\n",
            "effort.quick.steps",
            &["outside 1..=1000"],
        ),
        (
            "[effort.quick]\ndeadline = \"30s\"\n",
            "effort.quick.deadline",
            &["`30s`", "1m..=24h"],
        ),
        (
            "[effort.quick]\ndeadline = \"2d\"\n",
            "effort.quick.deadline",
            &["`2d`", "1m..=24h"],
        ),
        (
            "[effort.quick]\ndeadline = \"soon\"\n",
            "effort.quick.deadline",
            &["`soon` is not a duration"],
        ),
        (
            "[effort.quick]\nrecent_steps = 0\n",
            "effort.quick.recent_steps",
            &["outside 1..=50"],
        ),
        (
            "[effort.deep]\nbackend = \"big\"\nrecent_steps = 51\n",
            "effort.deep.recent_steps",
            &["outside 1..=50"],
        ),
        (
            "[policy.ceiling]\neffort_max = \"turbo\"\n",
            "policy.ceiling.effort_max",
            &["`turbo` is not an effort level"],
        ),
        (
            "[policy.profiles.workspace]\neffort_max = \"huge\"\n",
            "policy.profiles.workspace.effort_max",
            &["`huge` is not an effort level"],
        ),
        (
            "[policy.ceiling]\neffort_max = \"standard\"\n[policy.profiles.workspace]\neffort_max = \"deep\"\n",
            "policy.profiles.workspace.effort_max",
            &["`deep`", "`standard`", "policy.ceiling.effort_max"],
        ),
    ];
    for (tail, key, words) in cases {
        let (got_key, reason) = refused(&file(tail));
        assert!(
            got_key.ends_with(key),
            "the key {got_key:?} should be {key:?} ({tail})"
        );
        for word in *words {
            assert!(reason.contains(word), "{tail}: {reason:?} lacks {word:?}");
        }
    }
}

/// REQ-CFG-007: a level that names no backend runs on `default`, and without one a level that sets
/// a key is refused (`check` of a file without `default` still loads: REQ-CFG-002).
#[test]
fn a_level_with_keys_and_nowhere_to_run_is_refused() {
    let no_default = "config_version = 2\n[backends.big]\nkind = \"fake\"\n";
    // No effort section: it loads, as it did before levels existed.
    Config::parse(no_default).unwrap().validate().unwrap();
    // An empty header sets nothing.
    Config::parse(&format!("{no_default}[effort.standard]\n"))
        .unwrap()
        .validate()
        .unwrap();
    let (key, reason) = refused(&format!("{no_default}[effort.deep]\nsteps = 30\n"));
    assert_eq!(key, "effort.deep.backend");
    assert!(reason.contains("level `deep` names no backend"), "{reason}");
    assert!(reason.contains("`default`"), "{reason}");
    // The same level naming its backend loads.
    Config::parse(&format!(
        "{no_default}[effort.deep]\nbackend = \"big\"\nsteps = 30\n"
    ))
    .unwrap()
    .validate()
    .unwrap();
}

/// REQ-CFG-007 (RFC 015 Amendment 1, item 2): every level's backend is local, or every level's
/// backend has the same remote endpoint; the refusal names the levels and their endpoints.
#[test]
fn levels_are_all_local_or_all_on_one_remote_endpoint() {
    let remote = |name: &str, url: &str| {
        format!(
            "[backends.{name}]\nkind = \"openai_compatible\"\nurl = \"{url}\"\nmodel = \"m\"\nlocality = \"remote\"\n"
        )
    };
    let allowed = "[policy]\nremote_backend_allowed = true\n";
    // `default` is local, `deep` names a remote backend: refused, naming both levels.
    let (key, reason) = refused(&file(&format!(
        "{allowed}{}[effort.deep]\nbackend = \"far\"\n",
        remote("far", "https://models.example.org/v1")
    )));
    assert_eq!(key, "effort.deep.backend");
    // `quick` ships no backend either, so it is the first level that runs on `default`.
    assert!(
        reason.contains("`quick` (backend `default`, local, http://127.0.0.1:8080)"),
        "{reason}"
    );
    assert!(
        reason.contains("`deep` (backend `far`, remote, https://models.example.org:443)"),
        "{reason}"
    );
    // Two remote endpoints are not one.
    let two = "config_version = 2\n[policy]\nremote_backend_allowed = true\n";
    let (key, reason) = refused(&format!(
        "{two}{}{}[effort.deep]\nbackend = \"far\"\n",
        remote("default", "https://a.example.org/v1"),
        remote("far", "https://b.example.org/v1")
    ));
    assert_eq!(key, "effort.deep.backend");
    assert!(
        reason.contains("a.example.org") && reason.contains("b.example.org"),
        "{reason}"
    );
    // One remote endpoint, whatever the path, is allowed for every level.
    Config::parse(&format!(
        "{two}{}{}[effort.deep]\nbackend = \"far\"\n",
        remote("default", "https://a.example.org/v1"),
        remote("far", "https://a.example.org/other/v1")
    ))
    .unwrap()
    .validate()
    .unwrap();
    // A file of 0.12: one remote backend, no level section: it loads (`quick` ships on `default`).
    Config::parse(&format!(
        "{two}{}",
        remote("default", "https://a.example.org/v1")
    ))
    .unwrap()
    .validate()
    .unwrap();
    // A level that is refused when chosen sends nothing, so it does not count: `big` is local
    // and no level names it.
    Config::parse(&format!(
        "{two}{}[backends.big]\nkind = \"fake\"\n",
        remote("default", "https://a.example.org/v1")
    ))
    .unwrap()
    .validate()
    .unwrap();
}

/// REQ-TASK-013: `quick` ships `steps` 22 and `deadline` 10 minutes, nothing else; `standard` and
/// `deep` ship none; a key in the file replaces the shipped one, and only that key.
#[test]
fn shipped_keys_and_what_a_file_replaces() {
    let levels = Config::parse(&file("")).unwrap().effort_levels();
    assert_eq!(levels.quick.steps, Some(22));
    assert_eq!(levels.quick.deadline_ms, Some(10 * 60 * 1000));
    assert_eq!(levels.quick.backend, None);
    assert_eq!(levels.quick.context_chars, None);
    assert_eq!(levels.quick.recent_steps, None);
    assert!(levels.standard.is_empty());
    assert!(levels.deep.is_empty());
    assert!(!levels.is_configured(EffortLevel::Deep));
    assert!(levels.is_configured(EffortLevel::Standard));
    assert!(levels.is_configured(EffortLevel::Quick));

    let levels = Config::parse(&file("[effort.quick]\nsteps = 12\nbackend = \"big\"\n"))
        .unwrap()
        .effort_levels();
    assert_eq!(
        levels.quick.steps,
        Some(12),
        "the file replaces the shipped key"
    );
    assert_eq!(
        levels.quick.deadline_ms,
        Some(10 * 60 * 1000),
        "a key the file leaves out stays as shipped"
    );
    assert_eq!(levels.quick.backend.as_deref(), Some("big"));

    let levels = Config::parse(&file(
        "[effort.deep]\nbackend = \"big\"\ndeadline = \"4h\"\nrecent_steps = 3\n",
    ))
    .unwrap()
    .effort_levels();
    assert!(levels.is_configured(EffortLevel::Deep));
    assert_eq!(levels.deep.deadline_ms, Some(4 * 60 * 60 * 1000));
    assert_eq!(levels.deep.recent_steps, Some(3));
    assert_eq!(levels.deep.steps, None);
}

/// REQ-CFG-007: a valid configuration of 0.12 (version 1, one `[backend]`) loads unchanged: no
/// effort section, no `effort_max`, the shipped levels, and the policy it always had.
#[test]
fn a_configuration_of_0_12_loads_unchanged() {
    let text = "config_version = 1\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:8080/v1\"\nmodel = \"m\"\n[policy.profiles.workspace]\nsteps = 30\n";
    let loaded = Config::parse(text).unwrap();
    loaded.validate().unwrap();
    assert!(loaded.effort.is_empty());
    assert_eq!(loaded.effort_levels(), Levels::shipped());
    let policy = loaded.resolve_policy().unwrap();
    assert_eq!(policy.ceiling.effort_max, None);
    assert!(policy.profiles.values().all(|p| p.effort_max.is_none()));
    assert_eq!(policy.profile("workspace").unwrap().budgets.steps, 30);
}

/// REQ-CFG-001: what `check` prints reads back as the same configuration, the effort sections and
/// `effort_max` included.
#[test]
fn the_effort_sections_round_trip() {
    let config = Config::parse(&file(
        "[effort.quick]\nsteps = 12\ndeadline = \"5m\"\n[effort.deep]\nbackend = \"big\"\ncontext_chars = 20000\nrecent_steps = 8\n\
         [policy.ceiling]\neffort_max = \"deep\"\n[policy.profiles.workspace]\neffort_max = \"standard\"\n",
    ))
    .unwrap();
    config.validate().unwrap();
    let back = Config::parse(&config.to_toml()).unwrap();
    assert_eq!(back, config);
    let policy = config.resolve_policy().unwrap();
    assert_eq!(policy.ceiling.effort_max, Some(EffortLevel::Deep));
    assert_eq!(
        policy.profile("workspace").unwrap().effort_max,
        Some(EffortLevel::Standard)
    );
    assert_eq!(policy.profile("read_only").unwrap().effort_max, None);
}

/// An unknown key of an effort section is refused by name, as every section refuses one.
#[test]
fn an_unknown_effort_key_is_named() {
    let error = Config::parse(&file("[effort.quick]\nexecutions = 3\n")).unwrap_err();
    assert!(error.reason.contains("executions"), "{error}");
}
