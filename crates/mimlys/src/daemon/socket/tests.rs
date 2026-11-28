use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

fn dir_with_mode(base: &Path, name: &str, mode: u32) -> PathBuf {
    let dir = base.join(name);
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
    dir
}

/// External design §19.5: the socket is `d-<hash>.sock`, the hash of the data directory's
/// canonical path: one name per store, the same whichever way the directory is spelled.
#[test]
fn ed_19_5_the_socket_is_named_from_the_data_directorys_canonical_path() {
    let base = tempfile::tempdir().unwrap();
    let (a, b) = (base.path().join("a"), base.path().join("b"));
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let run = base.path().join("run");
    let first = socket_path(&run, &a).unwrap().unwrap();
    let name = first.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(
        name.starts_with("d-") && name.ends_with(".sock") && name.len() == 2 + 12 + 5,
        "{name}"
    );
    assert_eq!(first.parent(), Some(run.as_path()));
    // The same store, spelled another way, has the same socket.
    let spelled = a.join("..").join("a");
    assert_eq!(socket_path(&run, &spelled).unwrap().unwrap(), first);
    // Another store, another socket.
    assert_ne!(socket_path(&run, &b).unwrap().unwrap(), first);
    // A data directory that does not exist is served by no daemon.
    assert_eq!(socket_path(&run, &base.path().join("none")).unwrap(), None);
}

/// External design §19.5, §20: a path past the platform's limit for a socket is refused, saying
/// why, and never truncated.
#[test]
fn req_sec_016_a_socket_path_past_the_platforms_limit_is_refused_not_truncated() {
    let base = tempfile::tempdir().unwrap();
    let data = base.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let long = base.path().join("r".repeat(SUN_PATH_MAX));
    let refused = socket_path(&long, &data).unwrap_err();
    let SocketError::PathTooLong { path, len } = &refused else {
        panic!("{refused:?}")
    };
    assert!(*len > SUN_PATH_MAX);
    assert!(path.starts_with(&long));
    // The words say what it is and what limit it passed.
    let said = refused.to_string();
    assert!(said.contains(&SUN_PATH_MAX.to_string()), "{said}");
    assert!(said.contains(&len.to_string()), "{said}");
    // One byte under the limit is accepted: the limit is the platform's, exactly.
    let name_len = socket_name(&std::fs::canonicalize(&data).unwrap()).len();
    let fits = base
        .path()
        .join("r".repeat(SUN_PATH_MAX - name_len - 1 - base.path().as_os_str().len() - 1));
    assert!(socket_path(&fits, &data).unwrap().is_some());
}

/// REQ-SEC-016: the directory is checked: the user's own, mode 0700, not a link. A directory
/// that is not 0700 is refused (`mutation: the directory's mode not checked`).
#[test]
fn req_sec_016_a_directory_that_is_not_0700_is_refused() {
    let base = tempfile::tempdir().unwrap();
    let good = dir_with_mode(base.path(), "good", 0o700);
    assert_eq!(check_directory(&good), Ok(()));
    for mode in [0o755, 0o750, 0o770, 0o701, 0o500, 0o777] {
        let dir = dir_with_mode(base.path(), &format!("m{mode:o}"), mode);
        assert_eq!(
            check_directory(&dir),
            Err(SocketError::Directory {
                path: dir.clone(),
                fault: Fault::Mode(mode)
            }),
            "mode {mode:o}"
        );
    }
}

/// REQ-SEC-016: a directory of another user is refused, whatever its mode (the credentials are
/// injected: CI has no second user), and so are a link to a good directory, a file, and a
/// directory that is not there.
#[test]
fn req_sec_016_a_directory_that_is_not_the_users_own_is_refused() {
    let base = tempfile::tempdir().unwrap();
    let good = dir_with_mode(base.path(), "good", 0o700);
    // "Another user" owns it: the user the check is made for is not its owner.
    assert_eq!(
        check_directory_for(&good, own_uid() + 1),
        Err(SocketError::Directory {
            path: good.clone(),
            fault: Fault::OtherUser
        })
    );
    // A link to a good directory is not followed.
    let link = base.path().join("link");
    symlink(&good, &link).unwrap();
    assert_eq!(
        check_directory(&link),
        Err(SocketError::Directory {
            path: link.clone(),
            fault: Fault::Link
        })
    );
    // A file.
    let file = base.path().join("file");
    std::fs::write(&file, "x").unwrap();
    assert!(matches!(
        check_directory(&file),
        Err(SocketError::Directory {
            fault: Fault::WrongKind,
            ..
        })
    ));
    // Nothing there.
    assert!(matches!(
        check_directory(&base.path().join("none")),
        Err(SocketError::Directory {
            fault: Fault::Missing,
            ..
        })
    ));
    // The words say which, and that nothing was sent.
    let said = check_directory_for(&good, own_uid() + 1)
        .unwrap_err()
        .to_string();
    assert!(
        said.contains("another user") && said.contains("nothing"),
        "{said}"
    );
}

/// REQ-SEC-016: the socket's file is checked too: a socket of the user's own, mode 0600.
#[test]
fn req_sec_016_the_socket_file_is_the_users_own_socket_of_mode_0600() {
    let base = tempfile::tempdir().unwrap();
    let path = base.path().join("d-000000000000.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(check_socket_file(&path), Ok(()));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660)).unwrap();
    assert!(matches!(
        check_socket_file(&path),
        Err(SocketError::File {
            fault: Fault::Mode(0o660),
            ..
        })
    ));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        check_socket_file_for(&path, own_uid() + 1),
        Err(SocketError::File {
            fault: Fault::OtherUser,
            ..
        })
    ));
    // A regular file under the socket's name is not a socket.
    let plain = base.path().join("plain.sock");
    std::fs::write(&plain, "x").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        check_socket_file(&plain),
        Err(SocketError::File {
            fault: Fault::WrongKind,
            ..
        })
    ));
}

/// REQ-SEC-016, THR-39: the peer's uid is checked, by both ends: a peer of another uid is refused
/// (the credentials are injected). `mutation: the peer's uid not checked`.
#[test]
fn req_sec_016_a_peer_of_another_uid_is_refused() {
    let own = own_uid();
    let me = Peer {
        uid: own,
        pid: Some(4242),
    };
    assert_eq!(me.check(own), Ok(me));
    let other = Peer {
        uid: own + 1,
        pid: Some(4243),
    };
    assert_eq!(other.check(own), Err(SocketError::Peer { uid: own + 1 }));
    // Root is another user too: the check is equality, not "at most".
    assert!(Peer { uid: 0, pid: None }.check(1000).is_err());
}

/// REQ-SEC-016 (amended): the session is the peer's process as the system reports it; where the
/// system gives none, the record says that it is not known, and never a claim.
#[test]
fn req_sec_016_the_session_is_the_systems_word_and_unknown_when_it_gives_none() {
    let known = Peer {
        uid: 1,
        pid: Some(777),
    };
    assert_eq!(known.session_id(), "pid:777");
    let unknown = Peer { uid: 1, pid: None };
    assert_eq!(unknown.session_id(), "pid:unknown");
}

/// REQ-SEC-016, THR-39: a peer in a session led by one of the daemon's children is its tree; a
/// peer in another session is not; a peer whose session cannot be read is an error, which the
/// caller refuses.
#[test]
fn req_sec_016_a_peer_in_a_session_a_child_leads_is_the_daemons_own_tree() {
    let me = std::process::id();
    let session = rustix::process::getsid(None)
        .unwrap()
        .as_raw_nonzero()
        .get()
        .cast_unsigned();
    assert!(in_own_tree(me, &[session]).unwrap());
    assert!(in_own_tree(me, &[1, session, 2]).unwrap());
    assert!(!in_own_tree(me, &[session + 1]).unwrap());
    assert!(!in_own_tree(me, &[]).unwrap());
    // No such process: the session cannot be read, which is an error and not "not the tree".
    assert!(in_own_tree(i32::MAX as u32, &[session]).is_err());
}
