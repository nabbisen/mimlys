//! The context in the `tool_calls` transport (RFC 012; REQ-CTX-001,
//! REQ-CTX-002, REQ-CTX-003, REQ-CTX-010; external design §15.4), over the
//! fakes: the native turns of a history, the shape of a request, the budget's
//! count and the prefix measure in both transports.

use super::*;
use crate::grant::GrantSet;
use crate::native::{self, DecisionTransport, Function};
use crate::runtime::context::{
    ContextInputs, SYSTEM_INSTRUCTIONS, SYSTEM_INSTRUCTIONS_TOOL_CALLS, assemble,
};
use crate::spi::{Message, Role, ToolProvider};
use crate::step::Step;
use crate::task::{PlanVersion, Task};

fn native_harness(decisions: Vec<Value>) -> Harness {
    Harness::with_backend(
        Arc::new(ScriptedBackend::with_decisions(decisions).tool_calls()),
        Policy::shipped(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    )
}

fn script_reads(h: &Harness) {
    for _ in 0..4 {
        h.tools.script(
            "read_file",
            ExecutionOutcome::succeeded(json!({"content": "fn a() {}"}), 1),
        );
        h.tools.script(
            "list_dir",
            ExecutionOutcome::succeeded(json!({"entries": ["a.rs"]}), 1),
        );
    }
}

/// The persisted state a request is assembled from, so that a test can
/// assemble again with a part of it left out.
struct State {
    task: Task,
    plan: Option<PlanVersion>,
    steps: Vec<Step>,
    actions: Vec<crate::action::Action>,
    observations: Vec<crate::observation::Observation>,
    questions: Vec<crate::question::Question>,
    approvals: Vec<crate::approval::ApprovalRequest>,
    grants: GrantSet,
    tools: Vec<ToolDefinition>,
    now: crate::clock::Instant,
}

impl State {
    fn of(h: &Harness, id: TaskId) -> Self {
        use crate::clock::Clock;
        let now = h.clock.now();
        let mut grants = GrantSet::new();
        for g in h.persistence.load_grants(id).unwrap() {
            grants.add(g);
        }
        let names = grants.visible_tools(now);
        let tools = h
            .tools
            .definitions()
            .into_iter()
            .filter(|d| names.contains(&d.name))
            .collect();
        Self {
            task: h.runtime.get_task(id).unwrap().task,
            plan: h.persistence.load_plan(id).unwrap(),
            steps: h.persistence.load_steps(id).unwrap(),
            actions: h.persistence.load_actions(id).unwrap(),
            observations: h.persistence.load_observations(id).unwrap(),
            questions: h.persistence.load_questions(id).unwrap(),
            approvals: h.persistence.load_approvals(id).unwrap(),
            grants,
            tools,
            now,
        }
    }

    fn inputs<'a>(
        &'a self,
        transport: DecisionTransport,
        functions: &'a [Function],
        budget: usize,
    ) -> ContextInputs<'a> {
        ContextInputs {
            task: &self.task,
            tools: &self.tools,
            tool_definitions: "[]",
            grants: &self.grants,
            plan: self.plan.as_ref(),
            steps: &self.steps,
            actions: &self.actions,
            observations: &self.observations,
            questions: &self.questions,
            approvals: &self.approvals,
            history: None,
            unavailable: &[],
            budget,
            now: self.now,
            transport,
            functions,
        }
    }
}

/// The invariants of a native request: every call is answered by the very next
/// message, with the same id; no `tool` message answers anything else; and
/// the request does not end with an `assistant` message.
fn assert_native_shape(messages: &[Message]) {
    let mut open: Option<&str> = None;
    for (i, m) in messages.iter().enumerate() {
        match m.role {
            Role::Assistant => {
                let call = m
                    .call
                    .as_ref()
                    .unwrap_or_else(|| panic!("message {i}: an assistant message without a call"));
                assert!(
                    open.is_none(),
                    "message {i}: a call before the last was answered"
                );
                open = Some(&call.id);
            }
            Role::Tool => {
                let id = m.call_id.as_deref().expect("a tool message names its call");
                assert_eq!(
                    open.take(),
                    Some(id),
                    "message {i}: answers a different call"
                );
            }
            Role::System | Role::User => {
                assert!(open.is_none(), "message {i}: a call left unanswered");
                assert!(m.call.is_none() && m.call_id.is_none());
            }
        }
    }
    assert!(open.is_none(), "the request ends with an unanswered call");
    assert_ne!(
        messages.last().map(|m| m.role),
        Some(Role::Assistant),
        "a request never ends with an assistant message"
    );
}

/// REQ-CTX-001, external design §15.4: for a three-step history (a plan and two
/// actions) every call has its `tool` answer with the matching id, the labels are
/// intact, there is no decision request, and the last message is the last
/// answer, not the model's own.
#[test]
fn req_ctx_001_native_context_for_a_three_step_history() {
    let h = native_harness(vec![
        json!({"decision": "plan", "steps": ["read a.rs", "list the directory"]}),
        act("read_file", "a.rs"),
        json!({"decision": "act", "tool": "list_dir", "arguments": {"path": "."}}),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    script_reads(&h);
    let id = h.create();
    for _ in 0..4 {
        h.step(id);
    }
    let requests = h.backend.requests();
    assert_eq!(requests.len(), 4);
    for r in &requests {
        assert_eq!(r.transport, DecisionTransport::ToolCalls);
        assert_native_shape(&r.messages);
        assert!(
            r.messages
                .iter()
                .all(|m| !m.content.contains("decision_request")),
            "no decision-request segment"
        );
    }
    // The first request: the system message and the goal.
    let first = &requests[0].messages;
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].role, Role::System);
    assert!(first[0].content.starts_with(SYSTEM_INSTRUCTIONS_TOOL_CALLS));
    assert!(
        !first[0].content.contains("<<tool_definitions"),
        "the definitions travel as `tools`"
    );
    assert!(
        first[0]
            .content
            .contains("<<feedback kind=\"capabilities\">>")
    );
    assert!(first[1].content.starts_with("<<user kind=\"goal\">>"));
    assert_eq!(first[1].role, Role::User);

    // The last request: plan, then two actions, each call with its answer.
    let m = &requests[3].messages;
    assert_eq!(m.len(), 8, "{m:#?}");
    let call = |i: usize| m[i].call.as_ref().expect("a call");
    assert_eq!(
        (call(2).name.as_str(), call(2).id.as_str()),
        ("plan", "call_plan_1")
    );
    assert_eq!(
        call(2).arguments,
        r#"{"steps":["read a.rs","list the directory"]}"#
    );
    assert_eq!(m[3].call_id.as_deref(), Some("call_plan_1"));
    assert!(m[3].content.starts_with("<<plan version=\"1\">>"));
    assert_eq!(
        (call(4).name.as_str(), call(4).id.as_str()),
        ("read_file", "call_1")
    );
    assert_eq!(call(4).arguments, r#"{"path":"a.rs"}"#);
    assert_eq!(m[4].content, "", "a call carries no text of its own");
    assert_eq!(m[5].call_id.as_deref(), Some("call_1"));
    assert!(m[5].content.starts_with("<<observation tool=\"read_file\""));
    assert!(m[5].content.contains("fn a() {}"));
    assert_eq!(
        (call(6).name.as_str(), call(6).id.as_str()),
        ("list_dir", "call_2")
    );
    assert_eq!(m[7].call_id.as_deref(), Some("call_2"));
    assert!(m[7].content.starts_with("<<observation tool=\"list_dir\""));
    assert_eq!(m[7].role, Role::Tool);

    // The functions are the same in every request (the definitions are pinned).
    for r in &requests {
        assert_eq!(r.functions, requests[0].functions);
    }
    assert!(requests[0].functions.iter().any(|f| f.name == "read_file"));
}

/// REQ-CTX-001, REQ-DEC-009: a rejection of a decision that left no action is
/// runtime feedback as a `user` message (nothing of the call is recorded), and
/// says to call exactly one of the provided functions; the request still does
/// not end with an `assistant` message.
#[test]
fn req_ctx_001_a_rejection_tells_the_model_to_call_one_function() {
    let backend =
        Arc::new(ScriptedBackend::with_decisions(vec![act("no_such_tool", "a")]).tool_calls());
    backend.push(Ok(native::NO_DECISION.to_vec()));
    backend.push(Ok(serde_json::to_vec(&act("read_file", "a.rs")).unwrap()));
    let h = Harness::with_backend(
        backend,
        Policy::shipped(),
        vec![],
        crate::limits::LIM_C1_CONTEXT_CHARS,
    );
    script_reads(&h);
    let id = h.create();
    for _ in 0..3 {
        h.step(id);
    }
    let requests = h.backend.requests();
    // Step 0: an unknown tool; step 1 sees its rejection.
    let after_unknown = &requests[1].messages;
    assert_native_shape(after_unknown);
    let feedback = after_unknown.last().unwrap();
    assert_eq!(feedback.role, Role::User);
    assert!(
        feedback
            .content
            .starts_with("<<feedback kind=\"rejection\" code=\"unknown_tool\">>")
    );
    assert!(
        feedback
            .content
            .contains("Call one of the provided functions.")
    );
    // Step 1: no decision at all; step 2 sees `malformed`.
    let after_malformed = &requests[2].messages;
    assert_native_shape(after_malformed);
    let feedback = after_malformed.last().unwrap();
    assert!(feedback.content.contains("code=\"malformed\""));
    assert!(
        feedback
            .content
            .contains("Call exactly one of the provided functions"),
        "{}",
        feedback.content
    );
    assert!(
        !feedback
            .content
            .contains("JSON object with a `decision` field"),
        "the envelope is not described to the model"
    );
}

/// REQ-CTX-001: an answered question is an `ask_user` call and the user's
/// answer, labelled, as the `tool` message that answers it.
#[test]
fn req_ctx_001_a_question_and_its_answer_are_a_call_and_its_answer() {
    let h = native_harness(vec![
        json!({"decision": "ask_user", "question": "Which branch?"}),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    let id = h.create();
    h.step(id);
    h.runtime.answer_question(&h.session, id, "main").unwrap();
    h.step(id);
    let m = &h.backend.requests()[1].messages;
    assert_native_shape(m);
    let call = m[2].call.as_ref().unwrap();
    assert_eq!(call.name, "ask_user");
    assert_eq!(call.arguments, r#"{"question":"Which branch?"}"#);
    assert_eq!(m[3].call_id.as_deref(), Some(call.id.as_str()));
    assert!(m[3].content.starts_with("<<user kind=\"answer\">>"));
    assert!(m[3].content.contains("main"));
}

/// REQ-CTX-001: a call enters a request only with its answer. With the
/// observation of the last action, or the answer of a question, left out, the
/// call is left out too; no prefix of any history ends with an `assistant`
/// message or holds a call nobody answered.
#[test]
fn req_ctx_001_a_request_never_ends_with_an_assistant_message() {
    let h = native_harness(vec![
        json!({"decision": "plan", "steps": ["read"]}),
        act("read_file", "a.rs"),
        json!({"decision": "ask_user", "question": "Which?"}),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    script_reads(&h);
    let id = h.create();
    h.step(id);
    h.step(id);
    h.step(id);
    h.runtime.answer_question(&h.session, id, "this").unwrap();
    let mut state = State::of(&h, id);
    let functions = native::offer(&state.tools).functions;
    let budget = crate::limits::LIM_C1_CONTEXT_CHARS;
    let full = assemble(&state.inputs(DecisionTransport::ToolCalls, &functions, budget));
    assert_native_shape(&full.messages);
    let calls_in = |messages: &[Message]| messages.iter().filter(|m| m.call.is_some()).count();
    assert_eq!(calls_in(&full.messages), 3, "plan, read_file, ask_user");

    // The action has no observation: its call is not sent.
    let observations = std::mem::take(&mut state.observations);
    let without_observation =
        assemble(&state.inputs(DecisionTransport::ToolCalls, &functions, budget));
    assert_native_shape(&without_observation.messages);
    assert_eq!(calls_in(&without_observation.messages), 2, "plan, ask_user");
    state.observations = observations;

    // The question has no answer: its call is not sent.
    let questions = std::mem::take(&mut state.questions);
    let without_question =
        assemble(&state.inputs(DecisionTransport::ToolCalls, &functions, budget));
    assert_native_shape(&without_question.messages);
    assert_eq!(calls_in(&without_question.messages), 2, "plan, read_file");
    state.questions = questions;

    // Every prefix of the history.
    let all_steps = state.steps.clone();
    for k in 0..=all_steps.len() {
        state.steps = all_steps[..k].to_vec();
        let out = assemble(&state.inputs(DecisionTransport::ToolCalls, &functions, budget));
        assert_native_shape(&out.messages);
    }
}

/// REQ-CTX-003 (amended): the budget counts everything sent, the `tools` array
/// included. The same history costs the `tools` array more in the
/// `tool_calls` transport, and a budget the messages alone meet is exceeded
/// once the array is counted.
#[test]
fn req_ctx_003_the_tools_array_is_counted_in_the_budget() {
    let h = native_harness(vec![
        act("read_file", "a1.rs"),
        act("read_file", "a2.rs"),
        act("read_file", "a3.rs"),
        act("read_file", "a4.rs"),
        act("read_file", "a5.rs"),
        act("read_file", "a6.rs"),
        act("read_file", "a7.rs"),
    ]);
    script_reads(&h);
    let id = h.create();
    for _ in 0..6 {
        h.step(id);
    }
    let state = State::of(&h, id);
    let functions = native::offer(&state.tools).functions;
    let tools_chars = native::tools_chars(&functions);
    assert!(
        tools_chars > 1000,
        "the schemas are not small: {tools_chars}"
    );
    let roomy = crate::limits::LIM_C1_CONTEXT_CHARS;
    let counted = assemble(&state.inputs(DecisionTransport::ToolCalls, &functions, roomy));
    // Counted here from the wire: each message's text, and the name and arguments of its call.
    let messages_only: usize = counted
        .messages
        .iter()
        .map(|m| {
            m.content.chars().count()
                + m.call
                    .as_ref()
                    .map_or(0, |c| c.name.chars().count() + c.arguments.chars().count())
        })
        .sum();
    assert!(counted.messages.iter().any(|m| m.call.is_some()));
    assert_eq!(
        counted.measures.context_chars as usize,
        messages_only + tools_chars,
        "the count is the messages and the array"
    );
    assert_eq!(
        counted.measures.chars_before_fit as usize,
        messages_only + tools_chars
    );
    assert!(!counted.measures.over_budget);

    // A budget the messages meet by one character and the array does not.
    let tight = messages_only + 1;
    let squeezed = assemble(&state.inputs(DecisionTransport::ToolCalls, &functions, tight));
    assert!(
        squeezed.measures.over_budget || squeezed.measures.compressed_steps > 0,
        "the array tipped the budget: {:?}",
        squeezed.measures
    );
    // The envelope transport, with no functions, fits the same budget as before.
    let envelope = assemble(&state.inputs(DecisionTransport::Envelope, &[], roomy));
    assert_eq!(
        envelope.measures.context_chars as usize,
        crate::context::content_chars(&envelope.messages)
    );
}

/// REQ-CTX-010: the prefix shared with the previous request is recorded for
/// both transports; in `tool_calls` it covers the `tools` array, which is part
/// of what a prefix cache holds.
#[test]
fn req_ctx_010_prefix_stability_is_recorded_for_both_transports() {
    let run = |native: bool| {
        let decisions = vec![
            act("read_file", "a1.rs"),
            act("read_file", "a2.rs"),
            act("read_file", "a3.rs"),
        ];
        let scripted = ScriptedBackend::with_decisions(decisions);
        let backend = Arc::new(if native {
            scripted.tool_calls()
        } else {
            scripted
        });
        let h = Harness::with_backend(
            backend,
            Policy::shipped(),
            vec![],
            crate::limits::LIM_C1_CONTEXT_CHARS,
        );
        script_reads(&h);
        let id = h.create();
        for _ in 0..3 {
            h.step(id);
        }
        let first = h.backend.requests()[0].clone();
        let measures: Vec<_> = {
            h.persistence
                .load_steps(id)
                .unwrap()
                .into_iter()
                .map(|s| s.measures.expect("every decision request is measured"))
                .collect()
        };
        (first, measures)
    };
    for native in [false, true] {
        let (first, measures) = run(native);
        assert_eq!(
            measures[0].context.prefix_chars, 0,
            "nothing precedes the first"
        );
        // The segments that do not change: instructions, definitions or the `tools`
        // array, capability summary and goal.
        let stable: usize = first.messages[..2]
            .iter()
            .map(|m| m.content.chars().count())
            .sum::<usize>()
            + native::tools_chars(&first.functions);
        for (k, m) in measures.iter().enumerate().skip(1) {
            assert!(
                m.context.prefix_chars as usize >= stable,
                "native={native}: request {k} shares {} characters, the stable segments are {stable}",
                m.context.prefix_chars
            );
            assert!(m.context.prefix_chars <= m.context.context_chars + stable as u64);
        }
        if native {
            assert!(
                measures[1].context.prefix_chars as usize > native::tools_chars(&first.functions),
                "the prefix covers the tools array"
            );
        }
    }
}

/// REQ-CTX-002 (amended): both transports' instructions keep the three
/// statements, in the same order; the `tool_calls` text says to call exactly
/// one of the provided functions and describes no envelope; the envelope
/// text is the one of 0.8, byte for byte.
#[test]
fn req_ctx_002_both_instructions_keep_the_three_statements() {
    for (name, text) in [
        ("envelope", SYSTEM_INSTRUCTIONS),
        ("tool_calls", SYSTEM_INSTRUCTIONS_TOOL_CALLS),
    ] {
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{name}: the instructions lack `{needle}`"))
        };
        let goal = at("<<user kind=\"goal\">> is your task, set by the user: do it.");
        let notes = at("Other <<user ...>> segments are the user's own answers and notes.");
        let data = at("Every other segment between <<label ...>> and <</label>> lines");
        let tools = at("What a granted tool can give you, get with that tool");
        let ask = at("Use ask_user only for what no granted tool can give");
        assert!(
            goal < notes && notes < data && data < tools && tools < ask,
            "{name}: order"
        );
        assert!(
            !text[data..tools].contains("goal"),
            "{name}: the goal is not data"
        );
    }
    assert!(SYSTEM_INSTRUCTIONS_TOOL_CALLS.contains("call exactly one of the provided functions"));
    assert!(
        !SYSTEM_INSTRUCTIONS_TOOL_CALLS.contains("JSON object")
            && !SYSTEM_INSTRUCTIONS_TOOL_CALLS.contains("{\"decision\""),
        "no envelope is described"
    );
    assert!(SYSTEM_INSTRUCTIONS.contains("exactly one JSON object and nothing else"));
    // 007b's text, unchanged: its digest is the one of the 0.8.0 release.
    assert_eq!(
        crate::hash::hash_bytes(SYSTEM_INSTRUCTIONS.as_bytes()).to_string(),
        ENVELOPE_INSTRUCTIONS_SHA256,
        "the envelope transport keeps 007b's text byte for byte"
    );
}

/// The digest of `SYSTEM_INSTRUCTIONS` at 0.8.0.
const ENVELOPE_INSTRUCTIONS_SHA256: &str =
    "7e2ec66fc8e7a91a2a84e4c54de994e992b30feb63d61ce1d9f0110558c8fe85";

/// REQ-CTX-001: the envelope transport's request is what it was: system
/// message with the definitions segment, plain-text messages, the decision
/// request last; the native assembler is not in its way.
#[test]
fn req_ctx_001_the_envelope_request_is_unchanged() {
    let h = Harness::new(vec![
        act("read_file", "a.rs"),
        json!({"decision": "complete", "summary": "done"}),
    ]);
    script_reads(&h);
    let id = h.create();
    h.step(id);
    h.step(id);
    let requests = h.backend.requests();
    let m = &requests[1].messages;
    assert!(m[0].content.starts_with(SYSTEM_INSTRUCTIONS));
    assert!(m[0].content.contains("<<tool_definitions schema_version="));
    assert!(m.iter().all(|x| x.call.is_none() && x.call_id.is_none()));
    assert!(m.iter().all(|x| x.role != Role::Tool));
    assert_eq!(m[2].role, Role::Assistant);
    assert!(m[2].content.contains("\"decision\":\"act\""));
    assert!(m.last().unwrap().content.contains("decision_request"));
    assert!(requests[1].functions.is_empty());
}
