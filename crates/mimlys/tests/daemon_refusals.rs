//! What the daemon refuses at the hello (RFC 014, Amendment 1, items 7 and 8; external design
//! §19.5, §20; REQ-SEC-016): another runtime version, another configuration, another value of a
//! secret. Each as §20 says it, with nothing applied, and `stop` fixed for every version.
//!
//! Some tests speak the daemon's protocol directly (the library's `daemon::wire`), the shape the
//! hellos have for every version; others go through the command line. The test that makes a fake
//! daemon at the real socket is Unix's.

use mimlys::daemon::plat::Stream;
use mimlys::daemon::wire::{
    self, ClientHello, DaemonHello, HelloAnswer, RefusalKind, Reply, Request,
};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

mod common;
use common::run_bounded;

const KEY: &str = "MIMLYS_REFUSAL_TEST_KEY";
const SECRET_NAME: &str = "backends.default.api_key";

struct Env {
    registry: common::Registry,
    data: tempfile::TempDir,
    config: PathBuf,
}

impl Env {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let config = data.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "config_version = 1\n[runtime]\ndata_dir = {:?}\n[backend]\nkind = \"openai_compatible\"\nurl = \"http://127.0.0.1:9/v1\"\nmodel = \"m\"\napi_key = \"env:{KEY}\"\n",
                data.path().join("data").to_string_lossy(),
            ),
        )
        .unwrap();
        Self {
            registry: common::Registry::new(),
            data,
            config,
        }
    }

    fn command_with(&self, config: &std::path::Path, key: &str, args: &[&str]) -> Command {
        let mut cmd = common::mimlys_in(&self.registry);
        cmd.arg("--config")
            .arg(config)
            .args(args)
            .env("MIMLYS_CONFIG", "")
            .env("MIMLYS_WORKER_BIN", env!("CARGO_BIN_EXE_mimlys-worker"))
            .env(KEY, key)
            .stdin(std::process::Stdio::null());
        cmd
    }

    fn run_as(&self, config: &std::path::Path, key: &str, args: &[&str]) -> common::Bounded {
        run_bounded(&mut self.command_with(config, key, args), self.data.path())
    }

    fn run(&self, args: &[&str]) -> common::Bounded {
        self.run_as(&self.config, "secret-value-one", args)
    }

    fn start(&self) {
        let out = self.run(&["daemon", "start"]);
        assert_eq!(out.code(), 0, "{}\n{}", out.stdout, out.stderr);
    }

    /// A connection to the daemon (its socket, or its pipe), with a timeout on its reads.
    fn connect(&self) -> Stream {
        #[cfg(unix)]
        {
            let path = std::fs::read_dir(self.registry.path())
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|e| e == "sock"))
                .expect("the daemon's socket");
            let stream = Stream::connect(path).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            stream
        }
        #[cfg(windows)]
        {
            let status = self.run(&["--json", "daemon", "status"]);
            let value: Value = serde_json::from_str(status.stdout.trim()).unwrap();
            let name = value["daemon"]["socket"]
                .as_str()
                .expect("the daemon's pipe");
            let stream = Stream::connect(name, Duration::from_secs(30)).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            stream
        }
    }

    /// A connection that has read the daemon's hello, to answer as a client of its own making.
    fn greeted(&self) -> (Stream, DaemonHello) {
        let mut stream = self.connect();
        let hello: DaemonHello = wire::read(&mut stream).unwrap();
        (stream, hello)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.run(&["daemon", "stop"]);
    }
}

fn client_hello(
    hello: &DaemonHello,
    version: &str,
    config_hash: &str,
    secret: &str,
) -> ClientHello {
    let mut secrets = std::collections::BTreeMap::new();
    secrets.insert(
        SECRET_NAME.to_owned(),
        wire::keyed_hash(&hello.salt, SECRET_NAME, secret),
    );
    ClientHello {
        runtime_version: version.to_owned(),
        config_hash: config_hash.to_owned(),
        secrets,
    }
}

fn create_request() -> Request {
    use mimlys_core::api::{Envelope, Request as Control};
    use mimlys_core::ids::{RequestId, SystemIds};
    use mimlys_core::runtime::{CreateTask, Session};
    Request::Control {
        envelope: Box::new(Envelope {
            request_id: RequestId::new(&SystemIds),
            session: Session {
                id: "pid:1".into(),
                os_user: "me".into(),
            },
            request: Control::CreateTask(CreateTask {
                goal: "g".into(),
                scope: mimlys_core::task::ScopeSpec {
                    root: "/tmp".into(),
                    include: vec![],
                },
                profile: None,
                grants: vec![],
                budgets: None,
                deadline_ms: None,
                completion_check: None,
                hold: true,
                from_task: None,
                from_snapshot: None,
                plan_first: false,
                effort: None,
            }),
        }),
    }
}

/// REQ-SEC-016, external design §20: **a client of another runtime version is refused at the
/// hello**, the words naming both versions and `mimlys daemon restart`; the connection then reads
/// `stop` and nothing else. (`mutation: the hello's version not compared`)
#[test]
fn req_sec_016_a_client_of_another_version_is_refused_at_the_hello() {
    let env = Env::new();
    env.start();
    let (mut stream, hello) = env.greeted();
    wire::write(
        &mut stream,
        &client_hello(
            &hello,
            "0.0.1-other",
            &hello.config_hash,
            "secret-value-one",
        ),
    )
    .unwrap();
    let answer: HelloAnswer = wire::read(&mut stream).unwrap();
    assert!(!answer.accepted, "{answer:?}");
    let refusal = answer.refusal.expect("a refusal");
    assert_eq!(refusal.kind, RefusalKind::Version);
    assert!(
        refusal.message.contains("0.0.1-other"),
        "{}",
        refusal.message
    );
    assert!(
        refusal.message.contains(&hello.runtime_version),
        "{}",
        refusal.message
    );
    assert!(
        refusal.message.contains("mimlys daemon restart"),
        "{}",
        refusal.message
    );
    // Anything but `stop` is refused, and nothing is applied.
    wire::write(&mut stream, &create_request()).unwrap();
    let reply: Reply = wire::read(&mut stream).unwrap();
    assert!(matches!(reply, Reply::Refused { .. }), "{reply:?}");
    let (_, tasks) = {
        let out = env.run(&["--json", "task", "list"]);
        (
            out.code(),
            serde_json::from_str::<Value>(out.stdout.trim()).unwrap(),
        )
    };
    assert_eq!(tasks.as_array().map_or(0, Vec::len), 0, "{tasks}");
}

/// REQ-SEC-016: **a client of another version can stop the daemon**: `stop` is fixed for every
/// version, so that a release that cannot talk to the daemon can still end it.
#[test]
fn req_sec_016_stop_works_across_versions() {
    let env = Env::new();
    env.start();
    let refused = env.run_with_version(
        "0.0.1-other",
        &["task", "cancel", "00000000-0000-0000-0000-000000000000"],
    );
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(
        refused.stderr.contains("0.0.1-other") && refused.stderr.contains("mimlys daemon restart"),
        "{}",
        refused.stderr
    );
    let stopped = env.run_with_version("0.0.1-other", &["--json", "daemon", "stop"]);
    assert_eq!(stopped.code(), 0, "{}\n{}", stopped.stdout, stopped.stderr);
    assert!(
        std::fs::read_dir(env.registry.path())
            .unwrap()
            .flatten()
            .all(|e| e.path().extension().is_none_or(|x| x != "sock")),
        "the daemon ended"
    );
}

impl Env {
    fn run_with_version(&self, version: &str, args: &[&str]) -> common::Bounded {
        let mut cmd = self.command_with(&self.config, "secret-value-one", args);
        cmd.env(wire::PRETEND_VERSION_ENV, version);
        run_bounded(&mut cmd, self.data.path())
    }
}

/// REQ-SEC-016, external design §20: **a client whose configuration differs is refused what
/// changes state**, naming `mimlys daemon restart`; what only reads is not affected.
/// (`mutation: a changed configuration accepted`)
#[test]
fn req_sec_016_a_changed_configuration_is_refused_what_changes_state() {
    let env = Env::new();
    env.start();
    // On the wire: accepted, with the writes refused, and a creation answered `denied`.
    let (mut stream, hello) = env.greeted();
    wire::write(
        &mut stream,
        &client_hello(
            &hello,
            &hello.runtime_version,
            "not-the-daemons-hash",
            "secret-value-one",
        ),
    )
    .unwrap();
    let answer: HelloAnswer = wire::read(&mut stream).unwrap();
    assert!(answer.accepted, "{answer:?}");
    let refused = answer.writes_refused.expect("writes are refused");
    assert_eq!(refused.kind, RefusalKind::Configuration);
    assert!(refused.message.contains("mimlys daemon restart"));
    wire::write(&mut stream, &create_request()).unwrap();
    let Reply::Control { answer } = wire::read::<Reply>(&mut stream).unwrap() else {
        panic!("a control answer")
    };
    assert!(
        matches!(
            answer.outcome,
            mimlys_core::api::Outcome::Failed(mimlys_core::runtime::RuntimeError::Denied { .. })
        ),
        "{answer:?}"
    );
    // Through the command line: the same file, edited.
    let edited = env.data.path().join("edited.toml");
    let mut text = std::fs::read_to_string(&env.config).unwrap();
    text.push_str("# edited after the daemon started\n");
    std::fs::write(&edited, text).unwrap();
    let scope = env.data.path().join("scope");
    std::fs::create_dir_all(&scope).unwrap();
    let created = env.run_as(
        &edited,
        "secret-value-one",
        &[
            "task",
            "new",
            "--hold",
            "g",
            "--scope",
            scope.to_str().unwrap(),
        ],
    );
    assert_eq!(created.code(), 1, "{}\n{}", created.stdout, created.stderr);
    assert!(
        created.stderr.contains("mimlys daemon restart"),
        "{}",
        created.stderr
    );
    // A reader is not affected, and nothing was created.
    let listed = env.run_as(&edited, "secret-value-one", &["--json", "task", "list"]);
    assert_eq!(listed.code(), 0, "{}\n{}", listed.stdout, listed.stderr);
    assert_eq!(listed.stdout.trim(), "[]");
}

/// REQ-SEC-016, external design §19.5 (014c-6): **a refusal's word is read by any client.** A
/// daemon of a later release refuses in a word this one does not know; the client reads the whole
/// answer, shows the daemon's message (with the word), and refuses as for any refusal: it fails,
/// and applies nothing. (`mutation: RefusalKind a closed enum again` fails the read of the frame.)
#[test]
#[cfg(unix)]
fn req_sec_016_a_refusal_in_a_word_this_client_does_not_know_is_shown_and_refused() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    let env = Env::new();
    let data = env.data.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let run = env.registry.path();
    std::fs::create_dir_all(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = mimlys::daemon::socket::socket_path(&run, &data)
        .unwrap()
        .unwrap();
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    // A daemon of a later release: its hello is as ever; its answer refuses in a new word. One
    // connection for each command below; the thread is left to end with the process.
    let _daemon = std::thread::spawn(move || {
        for _ in 0..2 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let hello = DaemonHello {
                protocol: wire::PROTOCOL.to_owned(),
                hello_version: wire::HELLO_VERSION,
                runtime_version: wire::runtime_version().to_owned(),
                config_hash: "x".into(),
                salt: "ab".repeat(16),
            };
            let _ = wire::write(&mut stream, &hello);
            let _: Result<ClientHello, _> = wire::read(&mut stream);
            let _ = wire::write(
                &mut stream,
                &serde_json::json!({
                    "accepted": false,
                    "refusal": {
                        "kind": "a_word_of_a_later_release",
                        "message": "the daemon of a later release refuses this client",
                    },
                }),
            );
        }
    });
    for _ in 0..2 {
        // A command that changes a task goes through the daemon (a reader reads the store).
        let scope = env.data.path().join("scope");
        std::fs::create_dir_all(&scope).unwrap();
        let listed = env.run(&[
            "task",
            "new",
            "--hold",
            "g",
            "--scope",
            scope.to_str().unwrap(),
        ]);
        assert_eq!(listed.code(), 1, "{}\n{}", listed.stdout, listed.stderr);
        assert!(
            listed
                .stderr
                .contains("the daemon of a later release refuses this client"),
            "the daemon's own message is shown: {}",
            listed.stderr
        );
        assert!(
            listed.stderr.contains("a_word_of_a_later_release"),
            "and the word it used: {}",
            listed.stderr
        );
    }
}

/// REQ-SEC-016, external design §20: **a secret whose value differs** is refused what changes
/// state, naming the reference and never the value.
#[test]
fn req_sec_016_a_changed_secret_is_refused_naming_the_reference_and_never_the_value() {
    let env = Env::new();
    env.start();
    let scope = env.data.path().join("scope");
    std::fs::create_dir_all(&scope).unwrap();
    let refused = env.run_as(
        &env.config,
        "secret-value-TWO",
        &[
            "task",
            "new",
            "--hold",
            "g",
            "--scope",
            scope.to_str().unwrap(),
        ],
    );
    assert_eq!(refused.code(), 1, "{}\n{}", refused.stdout, refused.stderr);
    assert!(refused.stderr.contains(SECRET_NAME), "{}", refused.stderr);
    for text in [&refused.stdout, &refused.stderr] {
        assert!(
            !text.contains("secret-value"),
            "a value must never be shown: {text}"
        );
    }
    assert!(refused.stderr.contains("mimlys daemon restart"));
    // The same secret as the daemon's: accepted.
    let accepted = env.run(&[
        "task",
        "new",
        "--hold",
        "g",
        "--scope",
        scope.to_str().unwrap(),
    ]);
    assert_eq!(
        accepted.code(),
        0,
        "{}\n{}",
        accepted.stdout,
        accepted.stderr
    );
}
