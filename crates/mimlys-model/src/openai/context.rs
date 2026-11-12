//! The engine's context length and the context a request needs (REQ-MDL-003;
//! handoff 012c; limits LIM-C1, LIM-M2, LIM-M4).
//!
//! The probe reads the length where the engine says it, never from a guess,
//! beside the OpenAI-compatible prefix (`/v1`):
//!
//! - llama.cpp: `GET /props`, the per-slot `n_ctx` and the number of slots;
//! - Ollama: `GET /api/ps`, the `context_length` of the configured model when
//!   it is loaded (the value in force), else `POST /api/show`, the `num_ctx`
//!   of the model's parameters; else unknown (Ollama chooses the default when
//!   it loads the model, from the memory of the machine; `/api/show` does not
//!   report it, and its `model_info` holds the trained maximum, which is not
//!   the context in force).
//!
//! An engine that answers none of them is `unknown`. The need is the budget's
//! characters over the lowest characters-per-token measured in the transport
//! in force, plus the completion bound.

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

/// The engine that reported the context length, which decides the remedy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Answered `GET /props`: the length is set by `-c` at start, shared by its slots.
    LlamaCpp,
    /// Answered `GET /api/ps` or `POST /api/show`: the length is `num_ctx` of the model.
    Ollama,
}

/// What the engine said about its context.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineContext {
    /// The engine, when an answer was shaped as its own.
    pub engine: Option<Engine>,
    /// Tokens per request; `None` is unknown.
    pub tokens: Option<u64>,
    /// llama.cpp: the number of parallel slots `-c` is shared among.
    pub slots: Option<u64>,
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
/// top-level `n_ctx` shared out over `total_slots`; with `total_slots`.
pub fn from_llama_cpp_props(body: &Value) -> Option<(u64, u64)> {
    let slots = positive(body.get("total_slots")).unwrap_or(1);
    if let Some(n) = positive(body.pointer("/default_generation_settings/n_ctx")) {
        return Some((n, slots));
    }
    let total = positive(body.get("n_ctx"))?;
    Some(((total / slots).max(1), slots))
}

/// Ollama's `GET /api/ps`: the models in memory. `None` when the body is not
/// shaped as Ollama's (no `models` array); `Some(None)` when the configured
/// model (`is_configured` judges each listed name) is not loaded; else the
/// `context_length` it runs with.
pub fn from_ollama_ps(body: &Value, is_configured: impl Fn(&str) -> bool) -> Option<Option<u64>> {
    let models = body.get("models")?.as_array()?;
    Some(models.iter().find_map(|m| {
        let named = ["model", "name"]
            .iter()
            .filter_map(|k| m.get(*k).and_then(Value::as_str))
            .any(&is_configured);
        named.then(|| positive(m.get("context_length"))).flatten()
    }))
}

/// Ollama's `POST /api/show`: the `num_ctx` of the model's parameters (the
/// text of its Modelfile's `PARAMETER` lines, `num_ctx 16384`). `None` when
/// the body is not shaped as Ollama's answer, so another server's 200 is never
/// read as Ollama's; `Some(None)` when it sets none (or none that reads as a
/// positive number): the answer then says nothing about the context.
pub fn from_ollama_show(body: &Value) -> Option<Option<u64>> {
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
    let num_ctx = object
        .get("parameters")
        .and_then(Value::as_str)
        .and_then(|parameters| {
            parameters.lines().find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some("num_ctx"))
                    .then(|| words.next().and_then(|n| n.parse::<u64>().ok()))
                    .flatten()
                    .filter(|n| *n > 0)
            })
        });
    Some(num_ctx)
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

    /// REQ-MDL-003: llama.cpp's per-slot value first, with the slots, else the
    /// total over the slots.
    #[test]
    fn req_mdl_003_llama_cpp_props_shapes() {
        // Measured: `-c 8192 --parallel 2`.
        assert_eq!(
            from_llama_cpp_props(
                &json!({"default_generation_settings": {"n_ctx": 4096}, "total_slots": 2})
            ),
            Some((4096, 2))
        );
        assert_eq!(
            from_llama_cpp_props(&json!({"default_generation_settings": {"n_ctx": 8192}})),
            Some((8192, 1))
        );
        assert_eq!(
            from_llama_cpp_props(&json!({"n_ctx": 32768, "total_slots": 4})),
            Some((8192, 4))
        );
        assert_eq!(
            from_llama_cpp_props(&json!({"n_ctx": 4096})),
            Some((4096, 1))
        );
        assert_eq!(from_llama_cpp_props(&json!({"n_ctx": 0})), None);
        assert_eq!(from_llama_cpp_props(&json!({"n_ctx": "big"})), None);
        assert_eq!(from_llama_cpp_props(&json!({})), None);
    }

    /// REQ-MDL-003: Ollama's `/api/ps` gives the context in force of the
    /// configured model when it is loaded; another model's entry is not read;
    /// a body that is not Ollama's says nothing.
    #[test]
    fn req_mdl_003_ollama_ps_shapes() {
        let is_m = |name: &str| name == "m:latest" || name == "m";
        let loaded = json!({"models": [
            {"name": "other:latest", "model": "other:latest", "context_length": 2048},
            {"name": "m:latest", "model": "m:latest", "context_length": 4096},
        ]});
        assert_eq!(from_ollama_ps(&loaded, is_m), Some(Some(4096)));
        assert_eq!(
            from_ollama_ps(&json!({"models": []}), is_m),
            Some(None),
            "Ollama, nothing loaded"
        );
        assert_eq!(
            from_ollama_ps(
                &json!({"models": [{"name": "m:latest", "context_length": 0}]}),
                is_m
            ),
            Some(None)
        );
        assert_eq!(from_ollama_ps(&json!({"status": "ok"}), is_m), None);
        assert_eq!(from_ollama_ps(&json!([1]), is_m), None);
    }

    /// REQ-MDL-003: Ollama's `/api/show` gives `num_ctx` only where the
    /// parameters set it; the model's trained maximum in `model_info` is not
    /// the context in force, and no default is assumed.
    #[test]
    fn req_mdl_003_ollama_show_shapes() {
        let set =
            json!({"parameters": "stop \"<|im_end|>\"\nnum_ctx                        16384\n"});
        assert_eq!(from_ollama_show(&set), Some(Some(16_384)));
        let unset = json!({"parameters": "stop \"<|im_end|>\"", "details": {}});
        assert_eq!(from_ollama_show(&unset), Some(None));
        // Measured: no `parameters` at all, the trained maximum in `model_info`.
        let bare = json!({"modelfile": "FROM x", "model_info": {"qwen2.context_length": 131072}});
        assert_eq!(from_ollama_show(&bare), Some(None));
        assert_eq!(from_ollama_show(&json!({"unrelated": 1})), None);
        assert_eq!(from_ollama_show(&json!([1])), None);
        // Set but unreadable: unknown.
        assert_eq!(
            from_ollama_show(&json!({"parameters": "num_ctx lots"})),
            Some(None)
        );
    }
}
