use super::*;
use mimlys_core::decision::{Decision, parse_strict};
use mimlys_core::spi::{Message, Role};
use serde_json::json;

fn request() -> GenerateRequest {
    GenerateRequest {
        messages: vec![Message {
            role: Role::System,
            call: None,
            call_id: None,
            content: "s".into(),
        }],
        schema_version: 1,
        max_bytes: 1024,
        deadline_ms: 1000,
        transport: Default::default(),
        functions: vec![],
    }
}

/// REQ-MDL-002: decisions come back in order as parseable JSON; requests are recorded; exhaustion is an error.
#[tokio::test]
async fn req_mdl_002_scripted_decisions_in_order() {
    let b = FakeBackend::with_decisions(vec![
        json!({"decision": "plan", "steps": ["a"]}),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    let first = b.generate(request(), Cancellation::new()).await.unwrap();
    assert!(matches!(
        parse_strict(&first).unwrap(),
        Decision::Plan { .. }
    ));
    let second = b.generate(request(), Cancellation::new()).await.unwrap();
    assert!(matches!(
        parse_strict(&second).unwrap(),
        Decision::Complete { .. }
    ));
    assert_eq!(
        b.generate(request(), Cancellation::new()).await,
        Err(BackendError::ScriptExhausted)
    );
    assert_eq!(b.requests().len(), 3);
    assert_eq!(b.remaining(), 0);
    assert_eq!(b.locality(), Locality::Local);
}

/// REQ-MDL-002 / REQ-MDL-007: raw entries return verbatim, failures are simulated, output is bounded, cancellation observed.
#[tokio::test]
async fn req_mdl_002_control_entries() {
    let b = FakeBackend::new(vec![
        ScriptEntry::Control(ControlEntry {
            raw: Some("not json".into()),
            delay_ms: None,
            fail: None,
        }),
        ScriptEntry::Control(ControlEntry {
            raw: None,
            delay_ms: None,
            fail: Some("unavailable".into()),
        }),
        ScriptEntry::Control(ControlEntry {
            raw: Some("x".repeat(5000)),
            delay_ms: None,
            fail: None,
        }),
        ScriptEntry::Control(ControlEntry {
            raw: Some("late".into()),
            delay_ms: Some(1),
            fail: None,
        }),
    ]);
    assert_eq!(
        b.generate(request(), Cancellation::new()).await.unwrap(),
        b"not json"
    );
    assert!(matches!(
        b.generate(request(), Cancellation::new()).await,
        Err(BackendError::Unavailable { .. })
    ));
    let cut = b.generate(request(), Cancellation::new()).await.unwrap();
    assert_eq!(cut.len(), 1025);
    let cancel = Cancellation::new();
    cancel.cancel();
    assert_eq!(
        b.generate(request(), cancel).await,
        Err(BackendError::Cancelled)
    );
}

/// REQ-MDL-002: the script file format round-trips.
#[tokio::test]
async fn req_mdl_002_script_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script.json");
    std::fs::write(
        &path,
        r#"[{"decision":"give_up","reason":"no"},{"raw":"{","delay_ms":0}]"#,
    )
    .unwrap();
    let b = FakeBackend::from_script(&path).unwrap();
    assert_eq!(b.remaining(), 2);
    let first = b.generate(request(), Cancellation::new()).await.unwrap();
    assert!(matches!(
        parse_strict(&first).unwrap(),
        Decision::GiveUp { .. }
    ));
    assert_eq!(
        b.generate(request(), Cancellation::new()).await.unwrap(),
        b"{"
    );
}
