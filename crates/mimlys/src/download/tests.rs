//! The interpreter download's retry (REQ-SBX-001, external design §12,
//! LIM-O9, LIM-O10): the decision as a pure function, the driver over a
//! scripted `get`, and the real request against a local HTTP stand-in.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// REQ-SBX-001 (external design §12; LIM-O9, LIM-O10): the shipped policy is
/// the register's: 120 s per attempt, three retries after 1 s, 2 s, 4 s.
#[test]
fn req_sbx_001_the_shipped_policy_is_the_registers() {
    let p = Policy::shipped();
    assert_eq!(p.deadline, ms(120_000));
    assert_eq!(p.waits, vec![ms(1_000), ms(2_000), ms(4_000)]);
}

/// REQ-SBX-001 (external design §12): the transient failures are no
/// connection, a timeout, HTTP 429 and HTTP 5xx; nothing else is retried.
#[test]
fn req_sbx_001_which_failures_are_transient() {
    for code in [429, 500, 502, 503, 504, 599] {
        assert!(Failure::Status(code).is_transient(), "{code}");
    }
    for code in [301, 400, 401, 403, 404, 410, 451] {
        assert!(!Failure::Status(code).is_transient(), "{code}");
    }
    assert!(Failure::Transport("timed out".into()).is_transient());
    assert!(!Failure::Fatal("larger than LIM-S5".into()).is_transient());
}

/// REQ-SBX-001 (LIM-O10): the decision after each failure, with the waits in
/// order, and no retry beyond the policy's.
#[test]
fn req_sbx_001_the_retry_decision_follows_the_waits() {
    let p = Policy::shipped();
    let f = Failure::Status(504);
    assert_eq!(next(&f, 0, &p), Next::Retry(ms(1_000)));
    assert_eq!(next(&f, 1, &p), Next::Retry(ms(2_000)));
    assert_eq!(next(&f, 2, &p), Next::Retry(ms(4_000)));
    assert_eq!(next(&f, 3, &p), Next::Stop, "three retries, no fourth");
    assert_eq!(next(&Failure::Status(404), 0, &p), Next::Stop);
    assert_eq!(next(&Failure::Fatal("x".into()), 0, &p), Next::Stop);
}

/// A scripted `get` and the number of attempts it served.
struct Scripted {
    calls: Arc<Mutex<usize>>,
    script: std::vec::IntoIter<Result<Vec<u8>, Failure>>,
}

impl Scripted {
    fn get(&mut self, _url: &str, _deadline: Duration) -> Result<Vec<u8>, Failure> {
        *self.calls.lock().unwrap() += 1;
        self.script.next().expect("no more attempts than scripted")
    }
}

fn scripted(script: Vec<Result<Vec<u8>, Failure>>) -> (Scripted, Arc<Mutex<usize>>) {
    let calls = Arc::new(Mutex::new(0usize));
    (
        Scripted {
            calls: Arc::clone(&calls),
            script: script.into_iter(),
        },
        calls,
    )
}

/// REQ-SBX-001 (LIM-O10): 504 twice, then the artifact: installed after two
/// retries, with the waits 1 s and 2 s (recorded, not slept) and one log line
/// per retry.
#[test]
fn req_sbx_001_two_504s_then_the_artifact_is_downloaded_after_two_retries() {
    let (mut get, calls) = scripted(vec![
        Err(Failure::Status(504)),
        Err(Failure::Status(504)),
        Ok(b"artifact".to_vec()),
    ]);
    let mut waits = vec![];
    let mut lines = vec![];
    let got = fetch(
        "http://x/a.wasm",
        "javascript",
        &Policy::shipped(),
        |url, deadline| get.get(url, deadline),
        |w| waits.push(w),
        |l| lines.push(l.to_owned()),
    )
    .unwrap();
    assert_eq!(got, b"artifact");
    assert_eq!(*calls.lock().unwrap(), 3);
    assert_eq!(waits, vec![ms(1_000), ms(2_000)]);
    assert_eq!(lines.len(), 2, "one line per retry: {lines:?}");
    assert!(lines[0].contains("HTTP status 504") && lines[0].contains("retry 1 of 3"));
}

/// REQ-SBX-001 (external design §12): a 404 is not retried; the error is the
/// typed one, naming the URL, the status and the `--from FILE` remedy.
#[test]
fn req_sbx_001_a_404_is_not_retried_and_names_the_remedy() {
    let (mut get, calls) = scripted(vec![Err(Failure::Status(404))]);
    let mut waits = vec![];
    let err = fetch(
        "http://x/a.wasm",
        "javascript",
        &Policy::shipped(),
        |url, deadline| get.get(url, deadline),
        |w| waits.push(w),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(*calls.lock().unwrap(), 1);
    assert!(waits.is_empty());
    assert_eq!(err.last_status, Some(404));
    let text = err.to_string();
    assert!(text.contains("http://x/a.wasm"), "{text}");
    assert!(text.contains("HTTP status 404"), "{text}");
    assert!(
        text.contains("mimlys interpreters install javascript --from FILE"),
        "{text}"
    );
}

/// REQ-SBX-001 (LIM-O10): 504 every time: four attempts, three waits, then
/// the typed error with the last status.
#[test]
fn req_sbx_001_a_504_every_time_gives_up_after_the_retries() {
    let (mut get, calls) = scripted((0..4).map(|_| Err(Failure::Status(504))).collect());
    let mut waits = vec![];
    let err = fetch(
        "http://x/a.wasm",
        "javascript",
        &Policy::shipped(),
        |url, deadline| get.get(url, deadline),
        |w| waits.push(w),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(*calls.lock().unwrap(), 4);
    assert_eq!(waits, vec![ms(1_000), ms(2_000), ms(4_000)]);
    assert_eq!(err.retries, 3);
    assert_eq!(err.last_status, Some(504));
}

/// One connection per scripted answer; `None` accepts and never answers.
/// Returns the address and a counter of connections served.
fn stand_in(answers: Vec<Option<(u16, Vec<u8>)>>) -> (String, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/a.wasm", listener.local_addr().unwrap());
    let served = Arc::new(Mutex::new(0usize));
    let count = Arc::clone(&served);
    std::thread::spawn(move || {
        for answer in answers {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            *count.lock().unwrap() += 1;
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            match answer {
                Some((status, body)) => {
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
                // Holds the connection without answering, off this thread, so
                // that the next connection is served meanwhile.
                None => {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(1500));
                        drop(stream);
                    });
                }
            }
        }
    });
    (url, served)
}

fn quick() -> Policy {
    Policy {
        deadline: ms(400),
        waits: vec![ms(5), ms(5), ms(5)],
    }
}

/// REQ-SBX-001 (external design §12, LIM-O10): the real request against a
/// local HTTP stand-in that answers 504 twice and then the artifact.
#[test]
fn req_sbx_001_over_http_504_twice_then_the_artifact() {
    let (url, served) = stand_in(vec![
        Some((504, b"gateway".to_vec())),
        Some((504, b"gateway".to_vec())),
        Some((200, b"the artifact".to_vec())),
    ]);
    let got = fetch(
        &url,
        "javascript",
        &quick(),
        http_get,
        std::thread::sleep,
        |_| {},
    )
    .unwrap();
    assert_eq!(got, b"the artifact");
    assert_eq!(*served.lock().unwrap(), 3);
}

/// REQ-SBX-001: over HTTP a 404 is one request and the typed error.
#[test]
fn req_sbx_001_over_http_a_404_is_one_request() {
    let (url, served) = stand_in(vec![Some((404, b"no".to_vec()))]);
    let err = fetch(
        &url,
        "javascript",
        &quick(),
        http_get,
        std::thread::sleep,
        |_| {},
    )
    .unwrap_err();
    assert_eq!(*served.lock().unwrap(), 1);
    assert_eq!(err.last_status, Some(404));
}

/// REQ-SBX-001: over HTTP a 504 every time is the first request and
/// LIM-O10's three retries, then the typed error.
#[test]
fn req_sbx_001_over_http_a_504_every_time_gives_up() {
    let (url, served) = stand_in((0..4).map(|_| Some((504, b"g".to_vec()))).collect());
    let err = fetch(
        &url,
        "javascript",
        &quick(),
        http_get,
        std::thread::sleep,
        |_| {},
    )
    .unwrap_err();
    assert_eq!(*served.lock().unwrap(), 4);
    assert_eq!(err.retries, 3);
    assert_eq!(err.last_status, Some(504));
}

/// REQ-SBX-001 (LIM-O9): a server that never answers is cut at the deadline
/// of the attempt (not waited for), and is a transient failure, retried.
#[test]
fn req_sbx_001_over_http_a_silent_server_is_cut_at_the_deadline() {
    let (url, _served) = stand_in(vec![None, Some((200, b"late but there".to_vec()))]);
    let policy = Policy {
        deadline: ms(300),
        waits: vec![ms(5)],
    };
    let started = std::time::Instant::now();
    let got = fetch(
        &url,
        "javascript",
        &policy,
        http_get,
        std::thread::sleep,
        |_| {},
    )
    .unwrap();
    assert_eq!(got, b"late but there");
    assert!(
        started.elapsed() < Duration::from_millis(1400),
        "the first attempt was cut at its 300 ms deadline, not after the server's 1.5 s: {:?}",
        started.elapsed()
    );
}
