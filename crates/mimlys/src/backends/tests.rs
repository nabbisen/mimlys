use super::*;

/// A version-2 configuration with the backends written in `body`.
fn v2(body: &str) -> String {
    format!("config_version = 2\n{body}")
}

/// One `openai_compatible` backend named `name` at `url`, declared `locality`.
fn entry(name: &str, url: &str, locality: &str) -> String {
    format!(
        "[backends.{name}]\nkind = \"openai_compatible\"\nurl = {url:?}\nmodel = \"m\"\nlocality = \"{locality}\"\n"
    )
}

const ALLOW_REMOTE: &str = "[policy]\nremote_backend_allowed = true\n";

/// The load of a configuration text: parsed and validated.
fn load(text: &str) -> Result<Config, ConfigError> {
    let config = Config::parse(text)?;
    config.validate()?;
    Ok(config)
}

// ---- REQ-CFG-007: version 1 and version 2 ----

/// REQ-CFG-007: a version-1 file is read with its `[backend]` as `backends.default`, and the
/// configuration it makes is the version-2 form: the same value as the file written in
/// version 2, and what `check` prints reads back as itself.
#[test]
fn req_cfg_007_a_version_1_file_is_the_backend_named_default() {
    let one = "config_version = 1\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:8080/v1\"\nmodel = \"m\"\nmax_tokens = 2048\ncontext_chars = 12000\ntemperature = 0.2\n";
    let (config, deprecations) = Config::parse_noting(one).unwrap();
    assert_eq!(deprecations, [crate::config::Deprecation::ConfigVersion1]);
    assert_eq!(config.config_version, 2);
    assert_eq!(config.backend_names(), ["default"]);
    let default = config.default_backend().unwrap();
    assert_eq!(default.model.as_deref(), Some("m"));
    assert_eq!(default.max_tokens, 2048);
    config.validate().unwrap();

    let two = "config_version = 2\n[backends.default]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:8080/v1\"\nmodel = \"m\"\nmax_tokens = 2048\ncontext_chars = 12000\ntemperature = 0.2\n";
    let (same, none) = Config::parse_noting(two).unwrap();
    assert!(none.is_empty(), "version 2 has nothing deprecated");
    assert_eq!(same, config, "the two spellings are one configuration");
    // What `check` prints is the version-2 form, and reads back as the same value.
    let printed = config.to_toml();
    assert!(printed.contains("config_version = 2"), "{printed}");
    assert!(printed.contains("[backends.default]"), "{printed}");
    assert!(!printed.contains("[backend]"), "{printed}");
    assert_eq!(Config::parse(&printed).unwrap(), config);
}

/// REQ-CFG-007: a version-1 file with no `[backend]` had the default section, the fake backend:
/// it is `backends.default` too, so a 0.12 configuration of any shape loads as it did.
#[test]
fn req_cfg_007_a_version_1_file_without_a_backend_has_the_default_one() {
    let config = Config::parse("config_version = 1\n[runtime]\nadmission_slots = 3\n").unwrap();
    assert_eq!(config.backend_names(), ["default"]);
    assert_eq!(
        config.default_backend().unwrap(),
        &BackendSection::default()
    );
    config.validate().unwrap();
}

/// REQ-CFG-007: a version-2 file with `[backend]`, and a version-1 file with `[backends]`, are
/// refused, each naming the version that form needs.
#[test]
fn req_cfg_007_each_refused_mix_names_the_version() {
    let err = Config::parse("config_version = 2\n[backend]\nkind = \"fake\"\n").unwrap_err();
    assert_eq!(err.key, "backend");
    assert!(err.reason.contains("config_version 1"), "{err}");
    assert!(err.reason.contains("[backends.default]"), "{err}");

    let err =
        Config::parse("config_version = 1\n[backends.default]\nkind = \"fake\"\n").unwrap_err();
    assert_eq!(err.key, "backends");
    assert!(err.reason.contains("config_version 2"), "{err}");
    assert!(err.reason.contains("config_version = 2"), "{err}");
}

/// REQ-CFG-007: the name of a backend is lowercase ASCII, digits, `_` and `-`, at most 32
/// characters, starting with a letter or a digit; the refusal names the entry.
#[test]
fn req_cfg_007_a_backend_name_is_a_short_identifier() {
    for name in [
        "default",
        "small",
        "qwen-7b",
        "a_b",
        "7b",
        "x",
        &"n".repeat(32),
    ] {
        assert_eq!(name_problem(name), None, "{name}");
        let text = v2(&format!("[backends.\"{name}\"]\n"));
        load(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    for name in [
        "",
        "Default",
        "a b",
        "a.b",
        "-x",
        "_x",
        "naïve",
        "a/b",
        &"n".repeat(33),
    ] {
        assert!(name_problem(name).is_some(), "{name:?}");
        let text = v2(&format!("[backends.\"{name}\"]\n"));
        let err = load(&text).unwrap_err();
        assert_eq!(err.key, format!("backends.{name}"), "{name:?}");
        assert!(err.reason.contains("backend name"), "{err}");
    }
}

/// REQ-CFG-007, RFC 015 Amendment 1: no backend is named `default`: the lookup says so plainly,
/// lists the configured ones and picks none; a name that is not there says the same of that name.
#[test]
fn req_cfg_007_without_default_nothing_is_picked() {
    let config = load(&v2(&format!(
        "{}{}",
        entry("small", "http://127.0.0.1:11434/v1", "local"),
        entry("large", "http://127.0.0.1:8080/v1", "local")
    )))
    .unwrap();
    assert_eq!(config.backend_names(), ["large", "small"]);
    let err = config.default_backend().unwrap_err();
    assert!(
        err.reason.contains("no backend is named `default`"),
        "{err}"
    );
    assert!(err.reason.contains("`large`, `small`"), "{err}");
    assert!(config.backend("small").is_ok());
    let err = config.backend("nope").unwrap_err();
    assert!(err.reason.contains("no backend is named `nope`"), "{err}");
    assert!(err.reason.contains("`large`, `small`"), "{err}");

    let empty = load(&v2("")).unwrap();
    let err = empty.default_backend().unwrap_err();
    assert!(err.reason.contains("none is configured"), "{err}");
}

/// REQ-CFG-003: each backend's key is its own secret, named by its entry.
#[test]
fn req_cfg_007_each_backend_has_its_own_secret() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (dir.path().join("one"), dir.path().join("two"));
    std::fs::write(&one, "first-long-enough\n").unwrap();
    std::fs::write(&two, "second-long-enough\n").unwrap();
    let text = v2(&format!(
        "[backends.a]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:1/v1\"\nmodel = \"m\"\napi_key = 'file:{}'\n[backends.b]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:2/v1\"\nmodel = \"m\"\napi_key = 'file:{}'\n",
        one.display(),
        two.display()
    ));
    let secrets = load(&text).unwrap().resolve_secrets().unwrap();
    assert_eq!(
        secrets,
        vec![
            (
                "backends.a.api_key".to_owned(),
                "first-long-enough".to_owned()
            ),
            (
                "backends.b.api_key".to_owned(),
                "second-long-enough".to_owned()
            ),
        ]
    );
}

// ---- REQ-SEC-010: locality is checked against the URL ----

/// REQ-SEC-010: `local` is allowed only for a loopback host: `localhost`, `127.0.0.0/8`, `::1`,
/// read from the URL as written and never resolved; the edges.
#[test]
fn req_sec_010_local_is_allowed_for_a_loopback_host_only() {
    for url in [
        "http://localhost:8080/v1",
        "https://localhost/v1",
        // The host is not case-sensitive.
        "http://LOCALHOST:8080/v1",
        "http://127.0.0.1:8080/v1",
        "http://127.1.2.3:8080/v1",
        "http://127.255.255.254/v1",
        "http://[::1]:8080/v1",
        // A user name and a password are not the host.
        "http://user:pass@127.0.0.1:8080/v1",
        "http://me@localhost:8080/v1",
        // Judged as the HTTP client dials it: the URL parser's reading of a short, a
        // decimal and an octal form of 127.0.0.1.
        "http://127.1:8080/v1",
        "http://2130706433:8080/v1",
        "http://0177.0.0.1:8080/v1",
    ] {
        let text = v2(&entry("b", url, "local"));
        load(&text).unwrap_or_else(|e| panic!("{url} is loopback: {e}"));
        assert!(host_of(url).unwrap().loopback, "{url}");
    }
}

/// REQ-SEC-010: `local` with another host is refused, naming the backend and the host: a LAN
/// address, a name, and every form that looks loopback and is not.
#[test]
fn req_sec_010_local_with_another_host_is_refused_naming_it() {
    for (url, host) in [
        ("http://192.168.1.20:8080/v1", "192.168.1.20"),
        ("http://10.0.0.5/v1", "10.0.0.5"),
        ("https://api.example.org/v1", "api.example.org"),
        // `0.0.0.0` is not loopback: as a destination it is "this host" only by the kernel's
        // custom, and it is the address a server binds to listen on every interface.
        ("http://0.0.0.0:8080/v1", "0.0.0.0"),
        // An IPv4 address in an IPv6 form is not one of the three forms: write 127.0.0.1.
        ("http://[::ffff:127.0.0.1]:8080/v1", "[::ffff:7f00:1]"),
        ("http://[::]:8080/v1", "[::]"),
        ("http://[fe80::1]:8080/v1", "[fe80::1]"),
        // The name with a dot at its end, and a name under `localhost`, are other names: they are
        // resolved by a resolver, and the rule never resolves.
        ("http://localhost.:8080/v1", "localhost."),
        ("http://box.localhost:8080/v1", "box.localhost"),
        ("http://localhost.example.org/v1", "localhost.example.org"),
        ("http://127.0.0.1.nip.io/v1", "127.0.0.1.nip.io"),
        // The user name is not the host: this is `example.org`.
        ("http://127.0.0.1@example.org/v1", "example.org"),
        ("http://localhost:80@example.org/v1", "example.org"),
        ("http://user:pw@192.168.1.20/v1", "192.168.1.20"),
        // 128.0.0.1 is next to the block, not in it.
        ("http://128.0.0.1/v1", "128.0.0.1"),
        // Extra slashes are skipped: the host the client dials here is `v1`.
        ("http:///v1", "v1"),
    ] {
        let text = v2(&entry("lan", url, "local"));
        let err = load(&text).unwrap_err();
        assert_eq!(err.key, "backends.lan.locality", "{url}: {err}");
        assert!(err.reason.contains("backend `lan`"), "{url}: {err}");
        assert!(err.reason.contains("declared `local`"), "{url}: {err}");
        assert!(err.reason.contains(host), "{url}: {err}");
        assert!(!host_of(url).unwrap().loopback, "{url}");
        // What it says to do: declare it remote.
        assert!(err.reason.contains("declare it `remote`"), "{url}: {err}");
    }
}

/// REQ-SEC-010: a URL the rule cannot read is refused whatever the declaration says (there is
/// nothing to compare it with, and nothing could be sent to it), without echoing the URL, which
/// may carry a password: an IPv6 zone, a scheme that is not http, no host, no scheme.
#[test]
fn req_sec_010_a_url_without_a_readable_host_is_refused() {
    for url in [
        "http://[fe80::1%25eth0]:8080/v1",
        "http://[::1%eth0]:8080/v1",
        "ftp://127.0.0.1/v1",
        "file:///etc/passwd",
        "http://",
        "127.0.0.1:8080/v1",
        "localhost:8080/v1",
        "not a url",
        "",
    ] {
        for locality in ["local", "remote"] {
            let text = v2(&format!("{}{ALLOW_REMOTE}", entry("x", url, locality)));
            let err = load(&text).unwrap_err();
            assert_eq!(err.key, "backends.x.url", "{url}: {err}");
            assert!(err.reason.contains("backend `x`"), "{url}: {err}");
            if !url.is_empty() {
                assert!(!err.reason.contains(url), "the URL is not echoed: {err}");
            }
        }
    }
    let err = load(&v2(&entry(
        "x",
        "http://user:secret-pw@[::1%eth0]/v1",
        "local",
    )))
    .unwrap_err();
    assert!(!err.reason.contains("secret-pw"), "{err}");
}

/// REQ-SEC-010: `remote` is allowed for any host, a loopback one too (a tunnel to another
/// machine), when the administrator allows it; and not otherwise.
#[test]
fn req_sec_010_remote_may_be_loopback_and_needs_permission() {
    for url in [
        "http://127.0.0.1:9000/v1",
        "http://localhost:9000/v1",
        "http://[::1]:9000/v1",
        "http://192.168.1.20:8080/v1",
        "https://api.example.org/v1",
    ] {
        let remote = entry("far", url, "remote");
        load(&v2(&format!("{remote}{ALLOW_REMOTE}"))).unwrap_or_else(|e| panic!("{url}: {e}"));
        let err = load(&v2(&remote)).unwrap_err();
        assert_eq!(err.key, "backends.far.locality", "{url}");
        assert!(err.reason.contains("remote_backend_allowed"), "{err}");
    }
}

/// REQ-SEC-010: the `fake` backend has no URL and sends nothing anywhere: there is nothing to
/// compare its declaration with, and it takes none of an endpoint's keys.
#[test]
fn req_sec_010_the_fake_backend_has_no_url_to_check() {
    load(&v2("[backends.default]\nkind = \"fake\"\n")).unwrap();
    let err = load(&v2(
        "[backends.default]\nkind = \"fake\"\nurl = \"http://127.0.0.1/v1\"\n",
    ))
    .unwrap_err();
    assert_eq!(err.key, "backends.default.kind");
}

/// REQ-SEC-010: the check is on every entry, not on `default` alone.
#[test]
fn req_sec_010_every_entry_is_checked() {
    let text = v2(&format!(
        "{}{}",
        entry("default", "http://127.0.0.1:8080/v1", "local"),
        entry("lan", "http://192.168.1.20:8080/v1", "local")
    ));
    let err = load(&text).unwrap_err();
    assert_eq!(err.key, "backends.lan.locality");
}

/// REQ-SEC-010 (the same rule, from a version-1 file): a 0.12 configuration that declared a LAN
/// URL `local` stops loading and says why; naming the entry as the version-2 form.
#[test]
fn req_sec_010_a_version_1_file_with_a_lan_url_declared_local_stops_loading() {
    let text = "config_version = 1\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://192.168.1.20:8080/v1\"\nmodel = \"m\"\n";
    let err = load(text).unwrap_err();
    assert_eq!(err.key, "backends.default.locality");
    assert!(err.reason.contains("192.168.1.20"), "{err}");
}

// ---- REQ-CFG-007: entries on one endpoint agree on max_in_flight ----

fn with_limit(name: &str, url: &str, max_in_flight: usize) -> String {
    format!(
        "[backends.{name}]\nkind = \"openai_compatible\"\nurl = {url:?}\nmodel = \"{name}\"\nmax_in_flight = {max_in_flight}\n"
    )
}

/// REQ-CFG-007, LIM-O12: two entries on one endpoint with different `max_in_flight` are refused,
/// naming both entries and both values; the same value is fine; the endpoint is compared as the
/// engine gate compares it, so a path, a user name, the case of the host and a default port do
/// not make two endpoints.
#[test]
fn req_cfg_007_entries_on_one_endpoint_must_agree_on_max_in_flight() {
    for (a, b) in [
        ("http://127.0.0.1:11434/v1", "http://127.0.0.1:11434/v1"),
        (
            "http://127.0.0.1:11434/v1",
            "http://127.0.0.1:11434/other/path",
        ),
        (
            "http://127.0.0.1:11434/v1",
            "http://me:pw@127.0.0.1:11434/v1?x=1",
        ),
        ("http://LOCALHOST:11434/v1", "http://localhost:11434/v1"),
        ("http://localhost/v1", "http://localhost:80/v1"),
        ("https://localhost/v1", "https://localhost:443/v1"),
    ] {
        let text = |limit_b: usize| {
            v2(&format!(
                "{}{}",
                with_limit("big", a, 2),
                with_limit("small", b, limit_b)
            ))
        };
        load(&text(2)).unwrap_or_else(|e| panic!("{a} {b}: the same value is one gate: {e}"));
        let err = load(&text(3)).unwrap_err();
        assert_eq!(err.key, "backends.small.max_in_flight", "{a} {b}");
        assert!(
            err.reason.contains("`big`") && err.reason.contains("`small`"),
            "{err}"
        );
        assert!(err.reason.contains("max_in_flight 2 and 3"), "{err}");
        assert!(err.reason.contains("LIM-O12"), "{err}");
    }
}

/// REQ-CFG-007, LIM-O12: entries on different endpoints may differ: a port, a host, a scheme.
#[test]
fn req_cfg_007_entries_on_different_endpoints_may_differ() {
    let text = v2(&format!(
        "{}{}{}{}",
        with_limit("a", "http://127.0.0.1:11434/v1", 1),
        with_limit("b", "http://127.0.0.1:8080/v1", 4),
        with_limit("c", "http://localhost:11434/v1", 2),
        with_limit("d", "https://localhost:11434/v1", 3),
    ));
    load(&text).unwrap();
}

/// REQ-CFG-007, RFC 013 decision 7: two entries on one endpoint, agreeing, are one gate: the
/// endpoint they are keyed by is one.
#[test]
fn req_cfg_007_entries_on_one_endpoint_agreeing_are_one_gate() {
    let text = v2(&format!(
        "{}{}{}",
        with_limit("big", "http://127.0.0.1:11434/v1", 2),
        with_limit("small", "http://127.0.0.1:11434/v1", 2),
        with_limit("other", "http://127.0.0.1:8080/v1", 2),
    ));
    let (config, _) = Config::parse_noting(&text).unwrap();
    config.validate().unwrap();
    let loaded = Loaded {
        config,
        path: None,
        secrets: vec![],
        deprecations: vec![],
    };
    let endpoint = |name: &str| Backend::make(&loaded, name, None).unwrap().endpoint();
    assert!(endpoint("big").is_some());
    assert_eq!(endpoint("big"), endpoint("small"));
    assert_ne!(endpoint("big"), endpoint("other"));
    // A backend with no URL has none.
    let fake = Loaded {
        config: Config::default(),
        path: None,
        secrets: vec![],
        deprecations: vec![],
    };
    assert_eq!(Backend::make_default(&fake, None).unwrap().endpoint(), None);
}

/// The backend a task runs on: `default`; with none, the one that sends nothing and says why.
#[test]
fn req_cfg_007_make_default_is_default_or_absent() {
    let text = v2(&format!(
        "{}{}",
        entry("small", "http://127.0.0.1:11434/v1", "local"),
        entry("default", "http://127.0.0.1:8080/v1", "local"),
    ));
    let loaded = Loaded {
        config: load(&text).unwrap(),
        path: None,
        secrets: vec![],
        deprecations: vec![],
    };
    let made = Backend::make_default(&loaded, None).unwrap();
    assert_eq!(made.name, "default");
    assert_eq!(
        made.section.url.as_deref(),
        Some("http://127.0.0.1:8080/v1")
    );
    assert!(made.openai.is_some());
    let named = Backend::make(&loaded, "small", None).unwrap();
    assert_eq!(
        named.section.url.as_deref(),
        Some("http://127.0.0.1:11434/v1")
    );
    let AppError::Config(missing) = Backend::make(&loaded, "nope", None).err().unwrap() else {
        panic!("a name that is not there is a configuration error")
    };
    assert!(missing.reason.contains("`default`, `small`"), "{missing}");

    let none = Loaded {
        config: load(&v2("")).unwrap(),
        path: None,
        secrets: vec![],
        deprecations: vec![],
    };
    let absent = Backend::make_default(&none, None).unwrap();
    assert_eq!(absent.adapter.describe().kind, "none");
    assert!(none.config.default_backend().is_err());
}
