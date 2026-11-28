//! The pipe's own tests (Windows; run by CI's `windows-latest`). Each makes a pipe of a name of its
//! own, so that tests in parallel never meet.

use super::*;
use std::sync::atomic::AtomicU32;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn name(tag: &str) -> String {
    format!(
        r"\\.\pipe\mimlys-test-{}-{tag}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}

fn bound(tag: &str) -> (PipeListener, String) {
    let name = name(tag);
    let listener = PipeListener::bind(&name, &current_user_sid().unwrap()).unwrap();
    (listener, name)
}

/// A pipe, one client connected to it: both ends.
fn pair(tag: &str) -> (PipeListener, PipeStream, PipeStream) {
    let (listener, name) = bound(tag);
    let client = PipeStream::connect(&name, Duration::from_secs(10)).unwrap();
    let server = listener.accept().unwrap();
    (listener, server, client)
}

/// ED §19.5, REQ-SEC-016: **the pipe admits the user's SID alone**, and that is read back from the
/// system, not from what the code meant to set: one entry, the user's. (The mutation that makes the
/// pipe without its DACL, which takes the default, fails here: the default is no such list.)
#[test]
fn req_sec_016_the_pipes_dacl_holds_the_users_sid_alone_read_back() {
    let (listener, _) = bound("dacl");
    let sid = current_user_sid().unwrap();
    assert_eq!(listener.dacl_trustees().unwrap(), vec![sid]);
}

/// ED §19.5: **the pipe is the first instance of its name**: a process that already holds the name
/// (a squatter, in the daemon's place) makes the daemon's creation fail, and the name is the daemon's
/// again when the first instance is gone.
#[test]
fn req_sec_016_a_second_pipe_of_the_same_name_is_refused_while_the_first_lives() {
    let (first, name) = bound("squat");
    let sid = current_user_sid().unwrap();
    assert!(
        PipeListener::bind(&name, &sid).is_err(),
        "a second first instance of the name"
    );
    drop(first);
    assert!(PipeListener::bind(&name, &sid).is_ok());
}

/// A client and the daemon's end talk both ways, each is told the other's process (here, this
/// process), and that process's user is the user's.
#[test]
fn ed_19_5_both_ends_name_the_process_at_the_other_end_and_its_user() {
    let (_listener, mut server, mut client) = pair("talk");
    client.write_all(b"ping").unwrap();
    let mut buf = [0u8; 4];
    server.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"ping");
    server.write_all(b"pong").unwrap();
    client.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"pong");
    let me = std::process::id();
    assert_eq!(server.peer_process_id().unwrap(), me);
    assert_eq!(client.peer_process_id().unwrap(), me);
    let sid = current_user_sid().unwrap();
    assert_eq!(process_user_sid(me).unwrap(), sid);
}

/// Several clients at once: each `accept` hands over one, and the pipe is never without an
/// instance for the next.
#[test]
fn ed_19_5_several_clients_are_each_accepted() {
    let (listener, name) = bound("many");
    let listener = Arc::new(listener);
    let acceptor = {
        let listener = Arc::clone(&listener);
        std::thread::spawn(move || {
            (0..5)
                .map(|_| {
                    let mut stream = listener.accept().unwrap();
                    let mut byte = [0u8; 1];
                    stream.read_exact(&mut byte).unwrap();
                    byte[0]
                })
                .collect::<Vec<_>>()
        })
    };
    let clients: Vec<_> = (0..5u8)
        .map(|i| {
            let name = name.clone();
            std::thread::spawn(move || {
                let mut client = PipeStream::connect(&name, Duration::from_secs(20)).unwrap();
                client.write_all(&[i]).unwrap();
            })
        })
        .collect();
    for c in clients {
        c.join().unwrap();
    }
    let mut got = acceptor.join().unwrap();
    got.sort_unstable();
    assert_eq!(got, vec![0, 1, 2, 3, 4]);
}

/// A name with no pipe is `NotFound` (no daemon), not an error of another kind.
#[test]
fn ed_19_5_a_name_with_no_pipe_is_not_found() {
    let e = PipeStream::connect(&name("none"), Duration::from_secs(1)).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
}

/// A read that waits ends at its timeout, as a socket's does.
#[test]
fn ed_19_5_a_read_ends_at_its_timeout() {
    let (_listener, _server, mut client) = pair("timeout");
    client
        .set_read_timeout(Some(Duration::from_millis(150)))
        .unwrap();
    let begun = Instant::now();
    let e = client.read(&mut [0u8; 8]).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
    assert!(begun.elapsed() >= Duration::from_millis(100));
    // The connection is still good after a timeout.
    client.set_read_timeout(None).unwrap();
    let mut server = _server;
    server.write_all(b"x").unwrap();
    assert_eq!(client.read(&mut [0u8; 8]).unwrap(), 1);
}

/// **What a pipe whose peer has gone answers** (handoff 014d, item 1): what the peer wrote before it
/// went is read, then the end of the stream (`ERROR_BROKEN_PIPE` is `Ok(0)`), never an error; and a
/// write to it is a broken pipe. There is no option to set that could fail for it (the timeouts are
/// the daemon's own), so the macOS `EINVAL` has no counterpart here.
#[test]
fn ed_19_5_a_peer_that_has_gone_is_the_end_of_the_stream_and_a_broken_pipe() {
    let (_listener, mut server, mut client) = pair("gone");
    server.write_all(b"answer").unwrap();
    drop(server);
    // The options of a connection whose peer has closed are still set (they are ours).
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buf = [0u8; 16];
    let mut got = Vec::new();
    loop {
        match client.read(&mut buf).unwrap() {
            0 => break,
            n => got.extend_from_slice(&buf[..n]),
        }
    }
    assert_eq!(got, b"answer");
    let e = client.write_all(b"more").unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::BrokenPipe, "{e}");
    // The peer's process of a pipe that has gone cannot be read, or is this process still: either
    // way the callers refuse or go on by what it answers; it never panics.
    let _ = client.peer_process_id();
}

/// The same from the daemon's side: a client that went before it read.
#[test]
fn ed_19_5_a_client_that_has_gone_is_the_end_of_the_stream_for_the_daemon() {
    let (_listener, mut server, client) = pair("clientgone");
    drop(client);
    let mut buf = [0u8; 4];
    assert_eq!(server.read(&mut buf).unwrap(), 0);
    let e = server.write_all(b"x").unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::BrokenPipe, "{e}");
}

/// `shutdown` ends a read that waits in another thread, as the end of the stream.
#[test]
fn ed_19_5_a_shutdown_ends_a_read_that_waits_in_another_thread() {
    let (_listener, _server, client) = pair("shutdown");
    let reader = client.try_clone().unwrap();
    let waiting = std::thread::spawn(move || {
        let mut reader = reader;
        reader.read(&mut [0u8; 4])
    });
    std::thread::sleep(Duration::from_millis(200));
    client.shutdown(std::net::Shutdown::Both).unwrap();
    assert_eq!(waiting.join().unwrap().unwrap(), 0);
}

/// The stopper ends `accept` (the daemon's stop), now and every later one.
#[test]
fn ed_19_5_the_stopper_ends_accept() {
    let (listener, _) = bound("stopper");
    let stopper = listener.stopper();
    let listener = Arc::new(listener);
    let waiting = {
        let listener = Arc::clone(&listener);
        std::thread::spawn(move || listener.accept().map(|_| ()))
    };
    std::thread::sleep(Duration::from_millis(200));
    stopper.stop();
    let e = waiting.join().unwrap().unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    assert_eq!(
        listener.accept().map(|_| ()).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
}

/// **Own tree by Job Object** (REQ-SEC-016): a process in a job the daemon made is in it; this
/// process, which is in none of them, is not; and no job at all holds nobody.
#[test]
fn req_sec_016_a_process_in_a_job_is_in_it_and_this_process_is_not() {
    let mut child = std::process::Command::new("cmd.exe")
        .args(["/c", "ping -n 30 127.0.0.1 >nul"])
        .spawn()
        .unwrap();
    let mark = JobMark::put(child.id()).unwrap();
    assert!(!mark.is_empty());
    assert!(process_in_any_job(child.id(), &[mark.job()]).unwrap());
    assert!(!process_in_any_job(std::process::id(), &[mark.job()]).unwrap());
    assert!(!process_in_any_job(child.id(), &[]).unwrap());
    let _ = child.kill();
    let _ = child.wait();
    // A mark whose process has gone is empty.
    let begun = Instant::now();
    while !mark.is_empty() && begun.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(mark.is_empty());
}

/// A process that cannot be opened is an error, which the callers refuse (a pid that is not one).
#[test]
fn req_sec_016_a_process_that_cannot_be_opened_is_an_error() {
    assert!(process_user_sid(0xFFFF_FFF0).is_err());
    let mark_less = [JobRef::from_raw(0xFFFF_0000)];
    // An error of the system's about the job, never a fault of memory.
    assert!(process_in_any_job(std::process::id(), &mark_less).is_err());
}

/// Random bytes come from the system and differ.
#[test]
fn ed_19_5_random_bytes_differ() {
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    random_bytes(&mut a).unwrap();
    random_bytes(&mut b).unwrap();
    assert_ne!(a, b);
    assert_ne!(a, [0u8; 32]);
}
