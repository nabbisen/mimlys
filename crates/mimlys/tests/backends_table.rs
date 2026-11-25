//! `[backends.<name>]` and `config_version` 2 through the binary (REQ-CFG-007, REQ-SEC-010;
//! RFC 015 decision 3 and Amendment 1; handoff 015a, items 1 and 2): the deprecation line of a
//! version-1 file, the refused forms, the locality that is checked against the URL, the backend
//! that no command picks when none is named `default`.

use serde_json::Value;
use std::path::Path;

mod common;
use common::run_bounded;

struct Env {
    dir: tempfile::TempDir,
    config: std::path::PathBuf,
}

impl Env {
    /// A configuration file: `head` (the version line) and `body`, with the data directory.
    fn new(version: u32, body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = {version}\n[runtime]\ndata_dir = {:?}\n{body}",
                dir.path().join("data").to_string_lossy()
            ),
        )
        .unwrap();
        Self { dir, config }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut cmd = common::mimlys();
        cmd.arg("--config").arg(&self.config).args(args);
        cmd.env("MIMLYS_CONFIG", "");
        cmd.env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"));
        let out = run_bounded(&mut cmd, self.dir.path());
        (out.code(), out.stdout, out.stderr)
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let (code, stdout, stderr) = self.run(&all);
        let value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}\n{stderr}"));
        (code, value)
    }
}

fn demo_script() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/m1/demo.json")
        .to_string_lossy()
        .into_owned()
}

fn scope() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    dir
}

const DEPRECATED: &str = "deprecated: config_version = 1";

/// REQ-CFG-007: `check` of a version-1 file prints one deprecation line, which shows the
/// version-2 form, then the configuration in that form; `check` of a version-2 file prints none.
/// The 0.12 file runs as it did (the demonstration script, through `run`).
#[test]
fn req_cfg_007_check_says_once_that_version_1_is_deprecated() {
    let one = Env::new(
        1,
        &format!("[backend]\nkind = \"fake\"\nscript = {:?}\n", demo_script()),
    );
    let (code, stdout, stderr) = one.run(&["check"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.matches(DEPRECATED).count(), 1, "{stdout}");
    let line = stdout.lines().find(|l| l.contains(DEPRECATED)).unwrap();
    assert!(line.contains("[backends.default]"), "{line}");
    assert!(line.contains("config_version = 2"), "{line}");
    assert!(line.contains("0.14"), "{line}");
    // What follows the lines is the configuration, in the version-2 form.
    let printed = stdout
        .split_once("\n\n")
        .expect("a blank line before the configuration")
        .1;
    assert!(printed.starts_with("config_version = 2\n"), "{printed}");
    assert!(printed.contains("[backends.default]"), "{printed}");
    assert!(!printed.contains("[backend]"), "{printed}");
    let (code, value) = one.json(&["check"]);
    assert_eq!(code, 0);
    assert_eq!(
        value["deprecations"],
        serde_json::json!(["config_version_1"])
    );
    assert_eq!(value["config"]["config_version"], 2);
    assert_eq!(value["config"]["backends"]["default"]["kind"], "fake");

    let two = Env::new(
        2,
        &format!(
            "[backends.default]\nkind = \"fake\"\nscript = {:?}\n",
            demo_script()
        ),
    );
    let (code, stdout, stderr) = two.run(&["check"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!stdout.contains("deprecated"), "{stdout}");
    let (_, value) = two.json(&["check"]);
    assert_eq!(value["deprecations"], serde_json::json!([]));
    // The two spellings are one configuration.
    let (_, v1) = one.json(&["check"]);
    assert_eq!(v1["config"]["backends"], value["config"]["backends"]);
}

/// REQ-CFG-007: a version-1 file loads and runs as it did: the demonstration script, `run`
/// through the backend that is now named `default`.
#[test]
fn req_cfg_007_a_version_1_file_runs_on_the_backend_named_default() {
    let env = Env::new(
        1,
        &format!(
            "[backend]\nkind = \"fake\"\nscript = {:?}\n[policy.profiles.workspace]\napproval_tools = [\"write_file\"]\n",
            demo_script()
        ),
    );
    let scope = scope();
    let (code, stdout, stderr) =
        env.run(&["run", "fix it", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("waiting for your approval"), "{stdout}");
    assert!(
        !stdout.contains(DEPRECATED),
        "only `check` says it: {stdout}"
    );
    let (_, status) = env.json(&["status"]);
    assert_eq!(status["backend"]["kind"], "fake");
}

/// REQ-CFG-007: each refused mix is refused by every command that loads the configuration,
/// naming the version that form needs.
#[test]
fn req_cfg_007_a_form_of_the_other_version_is_refused_naming_the_version() {
    let v2_with_backend = Env::new(2, "[backend]\nkind = \"fake\"\n");
    for args in [&["check"][..], &["status"], &["task", "list"]] {
        let (code, _, stderr) = v2_with_backend.run(args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(stderr.contains("config_version 1"), "{stderr}");
        assert!(stderr.contains("[backends.default]"), "{stderr}");
    }
    let v1_with_backends = Env::new(1, "[backends.default]\nkind = \"fake\"\n");
    for args in [&["check"][..], &["status"], &["task", "list"]] {
        let (code, _, stderr) = v1_with_backends.run(args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(stderr.contains("config_version 2"), "{stderr}");
    }
    let (code, value) = v2_with_backend.json(&["check"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["kind"], "invalid_request");
    assert_eq!(value["error"]["detail"]["parameter"], "backend");
    // An empty `[backends]` header is the mix too (handoff 015a-5, item 5): by every command.
    let v1_with_empty_backends = Env::new(1, "[backends]\n");
    for args in [&["check"][..], &["status"], &["task", "list"]] {
        let (code, _, stderr) = v1_with_empty_backends.run(args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(stderr.contains("config_version 2"), "{stderr}");
    }
    let (code, value) = v1_with_empty_backends.json(&["check"]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["detail"]["parameter"], "backends");
}

/// REQ-SEC-010: a backend declared `local` whose URL's host is not loopback is refused at the
/// load, by every command, naming the backend and the host; declared `remote` (and allowed) it
/// loads, and so does a loopback URL declared `remote`.
#[test]
fn req_sec_010_a_lan_url_declared_local_is_refused_by_every_command() {
    let lan = "[backends.lan]\nkind = \"openai_compatible\"\nurl = \"http://192.168.1.20:8080/v1\"\nmodel = \"m\"\n";
    let env = Env::new(2, lan);
    for args in [
        &["check"][..],
        &["status"],
        &["task", "list"],
        &["run", "x"],
    ] {
        let (code, _, stderr) = env.run(args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(stderr.contains("backend `lan`"), "{stderr}");
        assert!(stderr.contains("192.168.1.20"), "{stderr}");
        assert!(stderr.contains("not loopback"), "{stderr}");
    }
    // A 0.12 file of the same meaning stops loading and says why.
    let old = Env::new(
        1,
        "[backend]\nkind = \"openai_compatible\"\nurl = \"http://192.168.1.20:8080/v1\"\nmodel = \"m\"\n",
    );
    let (code, _, stderr) = old.run(&["status"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("backends.default.locality"), "{stderr}");
    assert!(stderr.contains("192.168.1.20"), "{stderr}");

    // Declared remote: needs the permission, then loads.
    let remote = lan.replace("model = \"m\"", "model = \"m\"\nlocality = \"remote\"");
    let (code, _, stderr) = Env::new(2, &remote).run(&["check"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("remote_backend_allowed"), "{stderr}");
    let allowed = format!("{remote}[policy]\nremote_backend_allowed = true\n");
    let (code, _, stderr) = Env::new(2, &allowed).run(&["check"]);
    assert_eq!(code, 0, "{stderr}");
    // A tunnel: a loopback URL that is another machine.
    let tunnel = "[backends.tunnel]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:9000/v1\"\nmodel = \"m\"\nlocality = \"remote\"\n[policy]\nremote_backend_allowed = true\n";
    let (code, _, stderr) = Env::new(2, tunnel).run(&["check"]);
    assert_eq!(code, 0, "{stderr}");
}

/// REQ-CFG-007: two entries on one URL with different `max_in_flight` are refused, naming both.
#[test]
fn req_cfg_007_one_endpoint_with_two_limits_is_refused_by_the_load() {
    let body = |b: usize| {
        format!(
            "[backends.a]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:11434/v1\"\nmodel = \"x\"\nmax_in_flight = 2\n[backends.b]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:11434/v1\"\nmodel = \"y\"\nmax_in_flight = {b}\n"
        )
    };
    let (code, _, stderr) = Env::new(2, &body(3)).run(&["check"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("`a`") && stderr.contains("`b`"), "{stderr}");
    assert!(stderr.contains("max_in_flight"), "{stderr}");
    let (code, _, stderr) = Env::new(2, &body(2)).run(&["check"]);
    assert_eq!(code, 0, "{stderr}");
}

/// REQ-CFG-007, RFC 015 Amendment 1: with no backend named `default`, every command that runs
/// steps, and the one that creates a task, says plainly that there is none and lists the
/// configured ones; none is picked. `check` says the same and goes on; `status` and the commands
/// that run no step work.
#[test]
fn req_cfg_007_without_default_no_command_picks_a_backend() {
    let scope = scope();
    let env = Env::new(
        2,
        &format!(
            "[backends.small]\nkind = \"fake\"\nscript = {:?}\n[backends.large]\nkind = \"fake\"\nscript = {:?}\n",
            demo_script(),
            demo_script()
        ),
    );
    let scope_arg = scope.path().to_str().unwrap();
    for args in [
        vec!["run", "x", "--scope", scope_arg],
        vec!["task", "new", "x", "--scope", scope_arg],
        vec!["task", "new", "x", "--scope", scope_arg, "--hold"],
        vec!["task", "resume", "abcd1234"],
        vec!["approve", "abcd1234"],
        vec!["reject", "abcd1234"],
        vec!["answer", "abcd1234", "text"],
        // They can drive a step (a grant or a revocation that invalidates a pending approval sets
        // the task running, and the first command after a restart drives a recovered task).
        vec!["grant", "abcd1234", "tool", "read_file"],
        vec!["revoke", "abcd1234", "abcd1234"],
    ] {
        let (code, _, stderr) = env.run(&args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(
            stderr.contains("no backend is named `default`"),
            "{args:?}: {stderr}"
        );
        assert!(stderr.contains("`large`, `small`"), "{args:?}: {stderr}");
    }
    let (code, value) = env.json(&["run", "x", "--scope", scope_arg]);
    assert_eq!(code, 1);
    assert_eq!(value["error"]["kind"], "invalid_request");
    // Nothing was created.
    let (code, list) = env.json(&["task", "list"]);
    assert_eq!(code, 0);
    assert_eq!(list.as_array().unwrap().len(), 0);

    let (code, stdout, stderr) = env.run(&["check"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("no backend is named `default`"), "{stdout}");
    assert!(stdout.contains("`large`, `small`"), "{stdout}");
    let (code, stdout, stderr) = env.run(&["status"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("no backend is named `default`"), "{stdout}");
    // A command that runs no step needs no backend.
    let (code, _, stderr) = env.run(&["toolchain", "list"]);
    assert_eq!(code, 0, "{stderr}");
}

impl Env {
    /// The configuration written again with `body`, as an edit of the file would.
    fn rewrite(&self, version: u32, body: &str) {
        std::fs::write(
            &self.config,
            format!(
                "config_version = {version}\n[runtime]\ndata_dir = {:?}\n{body}",
                self.dir.path().join("data").to_string_lossy()
            ),
        )
        .unwrap();
    }
}

/// REQ-CFG-007 (handoff 015a-5, item 3): `grant` and `revoke` can drive a step, so they check for
/// `default` before anything is done: a task that waits for an approval, the file edited so that
/// no backend is named `default`, and the two commands are refused with the same words as the
/// others; the grant is not issued, the revocation is not made, the approval is still pending and
/// the script of the fake backend is not advanced (the task shows exactly as it did).
#[test]
fn req_cfg_007_grant_and_revoke_are_refused_before_anything_is_driven() {
    let scope = scope();
    let scope_arg = scope.path().to_str().unwrap();
    let configured = |name: &str| {
        format!(
            "[backends.{name}]\nkind = \"fake\"\nscript = {:?}\n[policy.profiles.workspace]\napproval_tools = [\"write_file\"]\n",
            demo_script()
        )
    };
    let env = Env::new(2, &configured("default"));
    let (code, value) = env.json(&["run", "fix it", "--scope", scope_arg]);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["task"]["state"], "waiting_user", "{value}");
    let id = value["task"]["task_id"].as_str().unwrap().to_owned();
    let grant = value["grants"][0]["grant_id"].as_str().unwrap().to_owned();
    // The time a task has taken so far is read from the clock (`cost.elapsed_ms`, from 0.13), so
    // two looks at a task that is not over differ by it and by nothing else.
    let without_elapsed = |mut shown: Value| {
        shown["cost"]
            .as_object_mut()
            .expect("a task shows its cost")
            .remove("elapsed_ms");
        shown
    };
    let (_, before) = env.json(&["task", "show", &id]);
    let before = without_elapsed(before);
    let (_, events_before) = env.json(&["events", &id]);

    env.rewrite(2, &configured("small"));
    for args in [
        vec!["grant", &id, "tool", "replace_in_file"],
        vec!["revoke", &id, &grant],
    ] {
        let (code, _, stderr) = env.run(&args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(
            stderr.contains("no backend is named `default`"),
            "{args:?}: {stderr}"
        );
        assert!(stderr.contains("`small`"), "{args:?}: {stderr}");
        let (code, value) = env.json(&args);
        assert_eq!(code, 1, "{args:?}: {value}");
        assert_eq!(value["error"]["kind"], "invalid_request", "{value}");
    }
    let (_, after) = env.json(&["task", "show", &id]);
    let after = without_elapsed(after);
    assert_eq!(after, before, "nothing was granted, revoked or driven");
    let (_, events_after) = env.json(&["events", &id]);
    assert_eq!(events_after, events_before, "no event was written");
}

/// REQ-CFG-007: a file with `[backends.default]` among others runs on it, and only on it.
#[test]
fn req_cfg_007_a_task_runs_on_the_backend_named_default() {
    let env = Env::new(
        2,
        &format!(
            "[backends.default]\nkind = \"fake\"\nscript = {:?}\n[backends.other]\nkind = \"fake\"\n[policy.profiles.workspace]\napproval_tools = [\"write_file\"]\n",
            demo_script()
        ),
    );
    let scope = scope();
    let (code, stdout, stderr) = env.run(&["run", "x", "--scope", scope.path().to_str().unwrap()]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("waiting for your approval"), "{stdout}");
}
