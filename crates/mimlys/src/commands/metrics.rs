//! `mimlys metrics [--task T]` (REQ-OBS-003).

use crate::app::{AppError, ReadApp};
use crate::messages;
use crate::output::Output;

/// Prints the derived metrics.
pub fn run(app: &ReadApp, task: Option<&str>, out: &Output) -> Result<(), AppError> {
    let task_id = match task {
        Some(t) => Some(app.resolve_task(t)?),
        None => None,
    };
    let metrics = app.reader.get_metrics(task_id)?;
    for (group, map) in [
        ("tasks_by_state", &metrics.tasks_by_state),
        ("events_by_name", &metrics.events_by_name),
        ("rejections_by_code", &metrics.rejections_by_code),
        ("halts_by_reason", &metrics.halts_by_reason),
        ("approvals_by_decision", &metrics.approvals_by_decision),
        ("actions_by_status", &metrics.actions_by_status),
    ] {
        for (k, v) in map {
            out.line(messages::metric_line(&format!("{group}.{k}"), *v));
        }
    }
    let c = &metrics.context;
    out.line(messages::metric_line(
        "context.measured_steps",
        c.measured_steps,
    ));
    if let Some(v) = c.context_chars_avg {
        out.line(messages::metric_line("context.context_chars_avg", v));
    }
    if let Some(v) = c.prefix_ratio {
        out.line(messages::metric_ratio("context.prefix_ratio", v));
    }
    if let Some(v) = c.compression_ratio {
        out.line(messages::metric_ratio("context.compression_ratio", v));
    }
    out.line(messages::metric_line(
        "context.compressed_steps",
        c.compressed_steps,
    ));
    out.line(messages::metric_line(
        "context.dropped_steps",
        c.dropped_steps,
    ));
    out.line(messages::metric_line(
        "context.over_budget_steps",
        c.over_budget_steps,
    ));
    if let Some(v) = c.model_ms_avg {
        out.line(messages::metric_line("model.ms_avg", v.max(0) as u64));
    }
    out.line(messages::metric_line(
        "model.prompt_tokens",
        c.prompt_tokens,
    ));
    out.line(messages::metric_line(
        "model.completion_tokens",
        c.completion_tokens,
    ));
    out.line(messages::metric_line(
        "model.cached_tokens",
        c.cached_tokens,
    ));
    if let Some(v) = c.chars_per_prompt_token {
        out.line(messages::metric_ratio("model.chars_per_prompt_token", v));
    }
    out.value(&metrics);
    Ok(())
}
