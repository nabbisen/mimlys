use super::*;
use crate::clock::{Clock, FakeClock};
use crate::ids::ScriptedIds;

fn question(text: &str) -> (Question, FakeClock) {
    let ids = ScriptedIds::default();
    let clock = FakeClock::default();
    let q = Question::ask(
        QuestionId::new(&ids),
        TaskId::new(&ids),
        StepId::new(&ids),
        text,
        clock.now(),
        None,
    );
    (q, clock)
}

/// REQ-APR-010: a question is sanitized and bounded; the answer is bounded, sanitized, non-empty, and one-shot.
#[test]
fn req_apr_010_question_and_answer() {
    let (mut q, clock) = question("Keep\u{200B} null?");
    assert_eq!(q.text, "Keep null?");
    assert_eq!(q.status, QuestionStatus::Pending);
    assert_eq!(
        q.answer("", Actor::Runtime, clock.now()),
        Err(QuestionError::AnswerEmpty)
    );
    let long = "a".repeat(LIM_D4_TEXT_CHARS + 1);
    assert_eq!(
        q.answer(&long, Actor::Runtime, clock.now()),
        Err(QuestionError::AnswerTooLong)
    );
    q.answer("Return 0\u{FEFF}.", Actor::Runtime, clock.now())
        .unwrap();
    assert_eq!(q.answer.as_deref(), Some("Return 0."));
    assert_eq!(q.status, QuestionStatus::Answered);
    assert_eq!(
        q.answer("again", Actor::Runtime, clock.now()),
        Err(QuestionError::NotPending)
    );
    let long_question = "q".repeat(LIM_D4_TEXT_CHARS + 50);
    let (q, _) = question(&long_question);
    assert_eq!(q.text.chars().count(), LIM_D4_TEXT_CHARS);
}

/// REQ-APR-006: a question expires like an approval, and only while pending.
#[test]
fn req_apr_006_question_expiry() {
    let (mut q, clock) = question("x?");
    assert!(!q.expire(clock.now()));
    q.expires_at = Some(clock.now().plus_millis(5).unwrap());
    clock.advance_millis(5);
    assert!(q.expire(clock.now()));
    assert_eq!(q.status, QuestionStatus::Expired);
    assert!(!q.expire(clock.now()));
    assert_eq!(
        q.answer("late", Actor::Runtime, clock.now()),
        Err(QuestionError::NotPending)
    );
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(json["status"], "expired");
    let back: Question = serde_json::from_value(json).unwrap();
    assert_eq!(back, q);
}
