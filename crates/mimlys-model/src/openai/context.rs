//! The engine's context length and the context a request needs (REQ-MDL-003;
//! handoff 012c; limits LIM-C1, LIM-M2, LIM-M4).
//!
//! The probe reads the length where the engine reports it, never from a
//! guess: llama.cpp's `GET /props` and Ollama's `POST /api/show`, both beside
//! the OpenAI-compatible prefix (`/v1`). An engine that answers neither is
//! `unknown`. The need is the budget's characters over the lowest
//! characters-per-token measured in the transport in force, plus the
//! completion bound.

use mimlys_core::native::DecisionTransport;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Characters per prompt token in the `tool_calls` transport: the lowest
/// ratio measured, on both engines, with the functions sent as `tools`
/// (the M4 report, section 7, run B: `chars_per_prompt_token` 3.35 to 3.60;
/// section 6, spike S2: 3.0 to 3.3 for the tools as JSON schema, before the
/// request was reshaped). Used for the need because the lowest ratio is the
/// largest number of tokens.
pub const CHARS_PER_TOKEN_TOOL_CALLS: f64 = 3.35;

/// Characters per prompt token in the `envelope` transport: the lowest ratio
/// measured (the M4 report, section 6, spike S2: `chars_per_prompt_token`
/// 4.32 to 4.38 for the envelope).
pub const CHARS_PER_TOKEN_ENVELOPE: f64 = 4.32;

/// The context an Ollama model runs with when its Modelfile sets no
/// `num_ctx`: 4 096 tokens (the M4 report, section 7: "Ollama's default of
/// 4 096 does not hold one request of this task"; the configuration chapter,
/// "Ollama and the context length"). A server may be started with another
/// default (`OLLAMA_CONTEXT_LENGTH`), which `/api/show` does not report.
pub const OLLAMA_DEFAULT_NUM_CTX: u64 = 4096;

/// The engine that reported the context length, which decides the remedy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Answered `GET /props`: the length is set by `-c` at start.
    LlamaCpp,
    /// Answered `POST /api/show`: the length is `num_ctx` of the model.
    Ollama,
}

/// The tokens a request can need: `context_chars` (LIM-C1, or the backend's
/// override) at the lowest characters-per-token measured for `transport`,
/// rounded up, plus `max_tokens` (LIM-M2, or the backend's override). With
/// the transport unsettled the lower ratio (the larger need) is used.
pub fn need_tokens(
    context_chars: usize,
    max_tokens: u32,
    transport: Option<DecisionTransport>,
) -> u64 {
    let ratio = match transport {
        Some(DecisionTransport::Envelope) => CHARS_PER_TOKEN_ENVELOPE,
        Some(DecisionTransport::ToolCalls) | None => CHARS_PER_TOKEN_TOOL_CALLS,
    };
    (context_chars as f64 / ratio).ceil() as u64 + u64::from(max_tokens)
}

/// Whether the engine's context cannot hold a request: known, and below the need.
pub fn is_too_small(context_tokens: Option<u64>, need: u64) -> bool {
    context_tokens.is_some_and(|n| n < need)
}

fn positive(value: Option<&Value>) -> Option<u64> {
    value.and_then(Value::as_u64).filter(|n| *n > 0)
}

/// llama.cpp's `GET /props`: the per-slot context
/// (`default_generation_settings.n_ctx`, what one request gets), else the
/// top-level `n_ctx` shared out over `total_slots`.
pub fn from_llama_cpp_props(body: &Value) -> Option<u64> {
    if let Some(n) = positive(body.pointer("/default_generation_settings/n_ctx")) {
        return Some(n);
    }
    let total = positive(body.get("n_ctx"))?;
    let slots = positive(body.get("total_slots")).unwrap_or(1);
    Some((total / slots).max(1))
}

/// Ollama's `POST /api/show`: the `num_ctx` of the model's parameters (the
/// text of its Modelfile's `PARAMETER` lines, `num_ctx 16384`), else
/// [`OLLAMA_DEFAULT_NUM_CTX`]. A body that is not shaped like Ollama's answer
/// says nothing (`None`), so another server's 200 is never read as Ollama's.
pub fn from_ollama_show(body: &Value) -> Option<u64> {
    let object = body.as_object()?;
    let shaped = [
        "modelfile",
        "parameters",
        "template",
        "details",
        "model_info",
    ]
    .iter()
    .any(|key| object.contains_key(*key));
    if !shaped {
        return None;
    }
    let line = object
        .get("parameters")
        .and_then(Value::as_str)
        .and_then(|parameters| {
            parameters
                .lines()
                .map(|line| line.split_whitespace())
                .find(|words| words.clone().next() == Some("num_ctx"))
        });
    match line {
        // Set, and readable; a value that is not a positive number is not read as a default.
        Some(mut words) => positive(
            words
                .nth(1)
                .and_then(|n| n.parse::<u64>().ok())
                .map(Value::from)
                .as_ref(),
        ),
        None => Some(OLLAMA_DEFAULT_NUM_CTX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mimlys_core::limits::{LIM_C1_CONTEXT_CHARS, LIM_M2_MAX_TOKENS};
    use serde_json::json;

    /// REQ-MDL-003: the need is the budget at the lowest measured ratio of
    /// the transport in force, plus the completion bound; the default budget
    /// in `tool_calls` is 13 649 tokens, in the envelope 11 504.
    #[test]
    fn req_mdl_003_need_by_transport() {
        assert_eq!(
            need_tokens(
                LIM_C1_CONTEXT_CHARS,
                LIM_M2_MAX_TOKENS,
                Some(DecisionTransport::ToolCalls)
            ),
            13_649
        );
        assert_eq!(
            need_tokens(
                LIM_C1_CONTEXT_CHARS,
                LIM_M2_MAX_TOKENS,
                Some(DecisionTransport::Envelope)
            ),
            11_504
        );
        // Unsettled: the larger need.
        assert_eq!(
            need_tokens(LIM_C1_CONTEXT_CHARS, LIM_M2_MAX_TOKENS, None),
            13_649
        );
        assert!(is_too_small(Some(4096), 13_649));
        assert!(!is_too_small(Some(16_384), 13_649));
        assert!(!is_too_small(Some(13_649), 13_649));
        assert!(!is_too_small(None, 13_649));
    }

    /// REQ-MDL-003: llama.cpp's per-slot value first, else the total over the slots.
    #[test]
    fn req_mdl_003_llama_cpp_props_shapes() {
        assert_eq!(
            from_llama_cpp_props(
                &json!({"default_generation_settings": {"n_ctx": 8192}, "n_ctx": 32768})
            ),
            Some(8192)
        );
        assert_eq!(
            from_llama_cpp_props(&json!({"n_ctx": 32768, "total_slots": 4})),
            Some(8192)
        );
        assert_eq!(from_llama_cpp_props(&json!({"n_ctx": 4096})), Some(4096));
        assert_eq!(from_llama_cpp_props(&json!({"n_ctx": 0})), None);
        assert_eq!(from_llama_cpp_props(&json!({"n_ctx": "big"})), None);
        assert_eq!(from_llama_cpp_props(&json!({})), None);
    }

    /// REQ-MDL-003: Ollama's `num_ctx`, else its default; not Ollama's shape, nothing.
    #[test]
    fn req_mdl_003_ollama_show_shapes() {
        let set =
            json!({"parameters": "stop \"<|im_end|>\"\nnum_ctx                        16384\n"});
        assert_eq!(from_ollama_show(&set), Some(16_384));
        let unset = json!({"parameters": "stop \"<|im_end|>\"", "details": {}});
        assert_eq!(from_ollama_show(&unset), Some(OLLAMA_DEFAULT_NUM_CTX));
        assert_eq!(
            from_ollama_show(&json!({"modelfile": "FROM x"})),
            Some(OLLAMA_DEFAULT_NUM_CTX)
        );
        assert_eq!(from_ollama_show(&json!({"unrelated": 1})), None);
        assert_eq!(from_ollama_show(&json!([1])), None);
        // Set but unreadable: unknown, not the default.
        assert_eq!(
            from_ollama_show(&json!({"parameters": "num_ctx lots"})),
            None
        );
    }
}
