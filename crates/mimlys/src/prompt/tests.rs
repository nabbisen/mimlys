use super::*;
use mimlys_core::approval::{Rendered, RenderedLine};
use mimlys_core::clock::Instant;
use mimlys_core::hash::hash_bytes;
use mimlys_core::ids::{ActionId, ApprovalId, ScriptedIds, TaskId};

fn request(reason: Option<&str>) -> ApprovalRequest {
    let ids = ScriptedIds::default();
    ApprovalRequest::new(
        ApprovalId::new(&ids),
        TaskId::new(&ids),
        ActionId::new(&ids),
        hash_bytes(b"action"),
        hash_bytes(b"grants"),
        Rendered {
            lines: vec![
                RenderedLine {
                    label: "Tool".into(),
                    value: "delete_path".into(),
                },
                RenderedLine {
                    label: "Arguments".into(),
                    value: "path = \"build\"".into(),
                },
            ],
        },
        reason,
        Instant::from_unix_millis(0).unwrap(),
        None,
    )
}

fn terminal(input: &str) -> Terminal<Buffered<std::io::Cursor<Vec<u8>>>, Vec<u8>> {
    Terminal::new(
        std::io::Cursor::new(input.as_bytes().to_vec()),
        Vec::new(),
        true,
    )
}

/// REQ-CLI-002: the prompt shows the rendered request in the fixed template
/// and reads the four options; the model's reason is marked untrusted and quoted.
#[test]
fn req_cli_002_template_and_options() {
    let req = request(Some("remove stale \u{202e}outputs"));
    let text = approval_text(&req, Some(7));
    assert!(text.starts_with("Approval required · task "));
    assert!(text.contains("· step 7\n"));
    assert!(text.contains("Tool"));
    assert!(text.contains("delete_path"));
    assert!(text.contains("Model's stated reason (untrusted): \"remove stale"));
    assert!(
        !text.contains('\u{202e}'),
        "bidirectional controls never reach the terminal"
    );
    assert!(text.ends_with(&format!("{}\n", messages::APPROVAL_OPTIONS)));

    let mut t = terminal("a\n");
    assert_eq!(t.approve(&req, None), Decision::Approve);
    let mut t = terminal("x\nr\nnot that file\n");
    assert_eq!(
        t.approve(&req, None),
        Decision::Reject {
            note: Some("not that file".into()),
            halt: false
        }
    );
    let mut t = terminal("h\n\n");
    assert_eq!(
        t.approve(&req, None),
        Decision::Reject {
            note: None,
            halt: true
        }
    );
    let mut t = terminal("g\ntool write_file\n");
    assert_eq!(
        t.approve(&req, None),
        Decision::Grant(GrantKind::Tool {
            name: "write_file".into()
        })
    );
    let mut t = terminal("");
    assert_eq!(t.approve(&req, None), Decision::Leave);
}

/// The grant syntax of the prompt and the command line.
#[test]
fn grant_syntax() {
    assert_eq!(
        parse_grant("tool:read_file").unwrap(),
        GrantKind::Tool {
            name: "read_file".into()
        }
    );
    assert_eq!(
        parse_grant("host_tool cargo_test").unwrap(),
        GrantKind::HostTool {
            name: "cargo_test".into()
        }
    );
    // Reserved kinds and the gateway by name are refused with the reason (external design §9.1).
    assert_eq!(
        parse_grant("host_read /home/u/docs"),
        Err(crate::config::HOST_PATH_GRANTS_RESERVED)
    );
    assert_eq!(
        parse_grant("tool run_host_tool"),
        Err(mimlys_core::host_tool::RUN_HOST_TOOL_NOT_A_GRANT)
    );
    assert_eq!(
        parse_grant("network:example.com:8443").unwrap(),
        GrantKind::Network {
            domains: vec!["example.com".into()],
            ports: vec![8443]
        }
    );
    assert_eq!(
        parse_grant("network example.com").unwrap(),
        GrantKind::Network {
            domains: vec!["example.com".into()],
            ports: vec![]
        }
    );
    assert!(parse_grant("tool").is_err());
    assert!(parse_grant("magic wand").is_err());
}

/// REQ-APR-010: a question is quoted, and an empty answer is not accepted.
#[test]
fn req_apr_010_question_prompt() {
    let ids = ScriptedIds::default();
    let q = Question::ask(
        mimlys_core::ids::QuestionId::new(&ids),
        TaskId::new(&ids),
        mimlys_core::ids::StepId::new(&ids),
        "Which\nbranch?",
        Instant::from_unix_millis(0).unwrap(),
        None,
    );
    let text = question_text(&q);
    assert!(text.contains("\"Which\\nbranch?\""));
    let mut t = terminal("\nmain\n");
    assert_eq!(t.answer(&q), Some("main".into()));
}

/// A prompt over a queue that a stop can be put to, with nothing read.
fn queued() -> (
    Terminal<Queue, Vec<u8>>,
    StopHandle,
    std::sync::mpsc::Sender<Input>,
) {
    let queue = Queue::new();
    let stop = queue.stop_handle().expect("a queue can be stopped");
    let lines = queue.sender();
    (Terminal::with_lines(queue, Vec::new(), true), stop, lines)
}

fn question() -> Question {
    let ids = ScriptedIds::default();
    Question::ask(
        mimlys_core::ids::QuestionId::new(&ids),
        TaskId::new(&ids),
        mimlys_core::ids::StepId::new(&ids),
        "Which branch?",
        Instant::from_unix_millis(0).unwrap(),
        None,
    )
}

/// REQ-CLI-005: a stop ends the wait at an approval prompt and at a question
/// prompt, with nothing decided and nothing answered.
#[test]
fn req_cli_005_a_stop_ends_the_wait_for_a_decision_or_an_answer() {
    let (mut t, stop, _) = queued();
    stop.stop();
    assert_eq!(t.approve(&request(None), None), Decision::Leave);
    let (mut t, stop, _) = queued();
    stop.stop();
    assert_eq!(t.answer(&question()), None);
}

/// REQ-CLI-005: a stop that comes while the prompt waits for the note of a
/// rejection, or for a grant, decides nothing: it is not a rejection without a note.
#[test]
fn req_cli_005_a_stop_during_a_rejection_or_a_grant_decides_nothing() {
    for typed in ["r", "h", "g"] {
        let (mut t, stop, lines) = queued();
        lines.send(Input::Line(typed.into())).unwrap();
        stop.stop();
        assert_eq!(t.approve(&request(None), None), Decision::Leave, "{typed}");
    }
}

/// REQ-CLI-005: once stopped, the prompt waits for nothing more; a line that
/// came before the stop is read.
#[test]
fn req_cli_005_a_stopped_prompt_waits_for_nothing_more() {
    let (mut t, stop, lines) = queued();
    lines.send(Input::Line("a".into())).unwrap();
    stop.stop();
    assert_eq!(t.approve(&request(None), None), Decision::Approve);
    assert_eq!(t.approve(&request(None), None), Decision::Leave);
    assert_eq!(
        t.approve(&request(None), None),
        Decision::Leave,
        "and again"
    );
    assert_eq!(t.answer(&question()), None);
}

/// REQ-CLI-005: the wait is ended by a stop that comes from another thread,
/// whether it comes before the prompt waits or after: the prompt returns, with
/// no line ever put to it. (The test reads the state, the returned decision,
/// and waits for it with a bound far above anything the stop needs.)
#[test]
fn req_cli_005_a_stop_from_another_thread_ends_a_wait_that_nothing_else_ends() {
    let (mut t, stop, _lines) = queued();
    let (done, finished) = std::sync::mpsc::channel();
    let waiting = std::thread::spawn(move || {
        let decision = t.approve(&request(None), None);
        let _ = done.send(decision);
    });
    stop.stop();
    let decision = finished
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the stop ended the wait");
    assert_eq!(decision, Decision::Leave);
    waiting.join().unwrap();
}

/// REQ-CLI-005: a prompt over a source that cannot be stopped has no handle,
/// so a channel that answers at once is never given one.
#[test]
fn req_cli_005_a_scripted_channel_has_no_stop_handle() {
    assert!(terminal("").stop_handle().is_none());
    let scripted = Scripted {
        decision: Decision::Approve,
        answer: String::new(),
    };
    assert!(scripted.stop_handle().is_none());
}
