use super::*;
use crate::config::Config;
use crate::daemon::socket;
use std::os::unix::fs::PermissionsExt;

fn loaded() -> Loaded {
    Config::load(None).unwrap()
}

fn private(dir: &std::path::Path) {
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// REQ-CLI-008, external design §20: with no registry directory, or none of the daemon's sockets
/// in it, there is no daemon, and the command runs embedded.
#[test]
fn req_cli_008_without_a_socket_there_is_no_daemon() {
    let base = tempfile::tempdir().unwrap();
    let data = base.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let run = base.path().join("run");
    assert!(matches!(
        reach(&run, &data, &loaded(), true),
        Ok(Reach::None)
    ));
    std::fs::create_dir(&run).unwrap();
    private(&run);
    assert!(matches!(
        reach(&run, &data, &loaded(), true),
        Ok(Reach::None)
    ));
}

/// REQ-SEC-016: a registry directory that is not 0700 is refused before anything is sent, when a
/// socket is in it; a directory with no socket holds no daemon, and is no reason to refuse a
/// command.
#[test]
fn req_sec_016_a_client_refuses_a_directory_that_is_not_0700() {
    let base = tempfile::tempdir().unwrap();
    let data = base.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let run = base.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        reach(&run, &data, &loaded(), true),
        Ok(Reach::None)
    ));
    let path = socket::socket_path(&run, &data).unwrap().unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let refused = reach(&run, &data, &loaded(), true).err().unwrap();
    assert!(matches!(
        refused,
        ReachError::Socket(SocketError::Directory {
            fault: socket::Fault::Mode(0o755),
            ..
        })
    ));
}

/// REQ-CLI-008, external design §20: a socket whose daemon is gone, with the store's lock free,
/// is removed, and the client says that the daemon is not running.
#[test]
fn req_cli_008_a_dead_daemons_socket_is_removed() {
    let base = tempfile::tempdir().unwrap();
    let data = base.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let run = base.path().join("run");
    std::fs::create_dir(&run).unwrap();
    private(&run);
    let path = socket::socket_path(&run, &data).unwrap().unwrap();
    // A socket file that nobody listens on: bound and dropped.
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(path.exists());
    // Asked not to remove it, the client leaves it.
    assert!(matches!(
        reach(&run, &data, &loaded(), false),
        Ok(Reach::Dead)
    ));
    assert!(path.exists(), "not removed when not asked");
    assert!(matches!(
        reach(&run, &data, &loaded(), true),
        Ok(Reach::Dead)
    ));
    assert!(!path.exists(), "the stale socket was removed");
    assert!(matches!(
        reach(&run, &data, &loaded(), true),
        Ok(Reach::None)
    ));
}

/// REQ-SEC-016, external design §19.5 (014c-6): **a client reads the answer of a daemon that
/// refused it in a word it does not know**, whole, whatever the word, and the daemon has closed
/// its end by the time the client has finished reading (the options of the socket are set after
/// the peer has gone, which macOS answers with `EINVAL`: the answer is read all the same). Many
/// times, since the order of the two ends is the system's.
#[test]
fn req_sec_016_a_client_reads_a_refusal_in_a_word_it_does_not_know_from_a_daemon_that_has_gone() {
    let base = tempfile::tempdir().unwrap();
    let data = base.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let run = base.path().join("run");
    std::fs::create_dir(&run).unwrap();
    private(&run);
    let path = socket::socket_path(&run, &data).unwrap().unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    const TIMES: usize = 200;
    let daemon = std::thread::spawn(move || {
        for _ in 0..TIMES {
            let (mut stream, _) = listener.accept().unwrap();
            let hello = DaemonHello {
                protocol: wire::PROTOCOL.to_owned(),
                hello_version: wire::HELLO_VERSION,
                runtime_version: wire::runtime_version().to_owned(),
                config_hash: "x".into(),
                salt: "ab".repeat(16),
            };
            wire::write(&mut stream, &hello).unwrap();
            let _: ClientHello = wire::read(&mut stream).unwrap();
            wire::write(
                &mut stream,
                &serde_json::json!({
                    "accepted": false,
                    "refusal": {"kind": "a_word_of_a_later_release", "message": "go away"},
                }),
            )
            .unwrap();
            // Closed at once: the client finds nobody there.
        }
    });
    for _ in 0..TIMES {
        let Ok(Reach::Daemon(daemon)) = reach(&run, &data, &loaded(), false) else {
            panic!("a daemon that refused: its answer is read")
        };
        assert!(!daemon.usable());
        let refusal = daemon.answer.refusal.expect("the refusal");
        assert_eq!(
            refusal.kind,
            wire::RefusalKind::Unknown("a_word_of_a_later_release".into())
        );
        let shown = refusal.shown();
        assert!(
            shown.contains("go away") && shown.contains("a_word_of_a_later_release"),
            "{shown}"
        );
    }
    daemon.join().unwrap();
}

/// REQ-SEC-016: a socket of mode 0660 is refused, and one that is not a socket.
#[test]
fn req_sec_016_a_client_refuses_a_socket_that_is_not_0600() {
    let base = tempfile::tempdir().unwrap();
    let data = base.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let run = base.path().join("run");
    std::fs::create_dir(&run).unwrap();
    private(&run);
    let path = socket::socket_path(&run, &data).unwrap().unwrap();
    let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660)).unwrap();
    assert!(matches!(
        reach(&run, &data, &loaded(), true).err().unwrap(),
        ReachError::Socket(SocketError::File {
            fault: socket::Fault::Mode(0o660),
            ..
        })
    ));
}
