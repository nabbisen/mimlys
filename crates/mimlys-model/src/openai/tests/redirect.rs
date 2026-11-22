//! A model backend never follows a redirect (REQ-SEC-010; handoff 015a-5, item 1): a server that
//! answers with one is a backend that failed, naming the host the redirect pointed to; nothing is
//! sent there.

use super::*;

/// The base of a server's URL, without its API prefix: where a redirect points. It is written
/// with the host `localhost`, so that the host a failure names is not the first server's own
/// (`127.0.0.1`); a client that followed would still reach the server (`localhost` resolves to
/// loopback), so a followed redirect shows in `second.requests()`.
fn base_of(second: &MockServer) -> String {
    second
        .url()
        .trim_end_matches("/v1")
        .replace("127.0.0.1", "localhost")
}

fn redirecting(second: &MockServer) -> MockServer {
    MockServer::start(
        MockConfig {
            redirect_to: Some(base_of(second)),
            ..MockConfig::default()
        },
        vec![],
    )
}

/// A decision request that is answered with a redirect fails as an unreachable backend does,
/// naming the host the `Location` points to; the second server receives nothing, whatever the
/// decision transport and the structured-output mode.
#[test]
fn req_sec_010_a_decision_request_is_never_redirected() {
    let rt = rt();
    for mode in [Mode::Schema, Mode::None, Mode::Auto] {
        let second = MockServer::start(MockConfig::default(), vec![]);
        let first = redirecting(&second);
        let backend = OpenAiBackend::new(config(first.url(), mode));
        let error = rt
            .block_on(backend.generate(request(), Cancellation::new()))
            .unwrap_err();
        assert_eq!(
            error,
            BackendError::Unavailable {
                detail: "redirect:localhost".into()
            },
            "{mode:?}"
        );
        assert!(
            second.requests().is_empty(),
            "{mode:?}: the redirect was followed: {:?}",
            second.requests()
        );
    }
    // The `tool_calls` transport asks the engine once (a request with a function) before the
    // decision: that request is not followed either, and it is not read as a refusal of `tools`.
    let second = MockServer::start(MockConfig::default(), vec![]);
    let first = redirecting(&second);
    let backend = OpenAiBackend::new(OpenAiConfig {
        decision_transport: TransportMode::ToolCalls,
        ..config(first.url(), Mode::Auto)
    });
    let mut native = request();
    native.transport = DecisionTransport::ToolCalls;
    let error = rt
        .block_on(backend.generate(native, Cancellation::new()))
        .unwrap_err();
    assert_eq!(
        error,
        BackendError::Unavailable {
            detail: "redirect:localhost".into()
        }
    );
    assert!(second.requests().is_empty(), "{:?}", second.requests());
}

/// The probe (`/v1/models`, and `/props` and `/api/ps` beside it) is not redirected: the backend
/// is unreachable, the detail names the host, and nothing reaches the second server.
#[test]
fn req_sec_010_the_probe_is_never_redirected() {
    let rt = rt();
    let second = MockServer::start(MockConfig::default(), vec![]);
    let first = redirecting(&second);
    let backend = OpenAiBackend::new(config(first.url(), Mode::Auto));
    let probe = rt.block_on(backend.probe());
    assert!(!probe.reachable);
    assert_eq!(probe.detail.as_deref(), Some("redirect:localhost"));
    assert_eq!(probe.model_listed, None);
    assert_eq!(probe.resident, None);
    assert_eq!(probe.context_tokens, None);
    assert!(second.requests().is_empty(), "{:?}", second.requests());
    // Each of the engine's own endpoints, asked alone: nothing is known, and nothing is followed.
    let deadline = Duration::from_secs(5);
    assert_eq!(rt.block_on(backend.read_resident(deadline)), None);
    let context = rt.block_on(backend.read_context(deadline));
    assert_eq!(context.tokens, None);
    assert_eq!(context.engine, None);
    assert!(second.requests().is_empty(), "{:?}", second.requests());
    // The first server did receive its requests: the failure is the redirect, not a dead server.
    assert!(!first.requests().is_empty());
}

/// Every status that asks a client to go elsewhere is a failure, and a `Location` that is relative,
/// missing, carries a password or is not a URL says no more than a host, or nothing: the password is
/// never shown.
#[test]
fn req_sec_010_a_redirect_names_the_host_only() {
    let rt = rt();
    let cases: [(u16, Option<&str>, &str); 7] = [
        (
            301,
            Some("http://other.example:9/v1/x?token=abc"),
            "redirect:other.example",
        ),
        (
            302,
            Some("https://user:hunter2@other.example/x"),
            "redirect:other.example",
        ),
        (
            303,
            Some("//proto-relative.example/x"),
            "redirect:proto-relative.example",
        ),
        (307, Some("/elsewhere"), "redirect:127.0.0.1"),
        (308, Some("http://[::1]:7/x"), "redirect:[::1]"),
        (301, None, "redirect"),
        (302, Some("http://exa mple/"), "redirect"),
    ];
    for (status, location, expected) in cases {
        let headers = location
            .map(|l| vec![("Location".to_owned(), l.to_owned())])
            .unwrap_or_default();
        let server = MockServer::start(
            MockConfig::default(),
            vec![Reply::Raw {
                status,
                body: "{}".into(),
                headers,
            }],
        );
        let backend = OpenAiBackend::new(config(server.url(), Mode::Schema));
        let error = rt
            .block_on(backend.generate(request(), Cancellation::new()))
            .unwrap_err();
        assert_eq!(
            error,
            BackendError::Unavailable {
                detail: expected.into()
            },
            "{status} {location:?}"
        );
        let text = format!("{error:?}");
        assert!(
            !text.contains("hunter2") && !text.contains("token"),
            "{text}"
        );
    }
}

/// A `304` and the other `3xx` that are not redirects are not read as one: they are a status, as
/// before.
#[test]
fn req_sec_010_a_status_that_is_not_a_redirect_is_a_status() {
    let rt = rt();
    let server = MockServer::start(
        MockConfig::default(),
        vec![Reply::Raw {
            status: 304,
            body: String::new(),
            headers: vec![("Location".to_owned(), "http://other.example/".to_owned())],
        }],
    );
    let backend = OpenAiBackend::new(config(server.url(), Mode::Schema));
    let error = rt
        .block_on(backend.generate(request(), Cancellation::new()))
        .unwrap_err();
    assert_eq!(
        error,
        BackendError::Unavailable {
            detail: "status:304".into()
        }
    );
}
