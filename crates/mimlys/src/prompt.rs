//! The approval and question prompts (external design §10.1; REQ-CLI-002,
//! REQ-APR-009). The prompt prints the runtime's rendered request through a
//! fixed template and nothing else from the task; it reads one of `a`, `r`,
//! `h`, `g`. The same `Interaction` trait lets the test harness answer
//! without a terminal (the fake approval channel of REQ-TEST-002).

use crate::messages;
use mimlys_core::approval::ApprovalRequest;
use mimlys_core::grant::GrantKind;
use mimlys_core::limits::LIM_D2_REASON_CHARS;
use mimlys_core::question::Question;
use std::io::{BufRead, Write};

/// What the user decided at a prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Approve the action.
    Approve,
    /// Reject it, with an optional note; `halt` ends the task.
    Reject {
        /// Note for the model.
        note: Option<String>,
        /// Reject and halt.
        halt: bool,
    },
    /// Issue a grant; the prompt is shown again afterwards.
    Grant(GrantKind),
    /// Input ended; leave the task waiting.
    Leave,
}

/// A channel to the user.
pub trait Interaction {
    /// Whether prompts can be shown at all.
    fn interactive(&self) -> bool;
    /// Asks for a decision on an approval request.
    fn approve(&mut self, request: &ApprovalRequest, step_index: Option<u32>) -> Decision;
    /// Asks for the answer to a question; `None` leaves the task waiting.
    fn answer(&mut self, question: &Question) -> Option<String>;
    /// Asks a yes-or-no question (promotion, deletion); `false` on anything but `y`.
    fn confirm(&mut self, question: &str) -> bool;
}

/// The fixed template around the rendered request (REQ-CLI-002).
pub fn approval_text(request: &ApprovalRequest, step_index: Option<u32>) -> String {
    let mut text = messages::approval_header(&request.task_id.to_string(), step_index);
    text.push('\n');
    text.push_str(&request.display_text());
    text.push('\n');
    text.push_str(messages::APPROVAL_OPTIONS);
    text.push('\n');
    text
}

/// The question template: the model's text is JSON-quoted, never raw.
pub fn question_text(question: &Question) -> String {
    format!(
        "{}\n{}\n",
        messages::question_header(&question.task_id.to_string()),
        serde_json::to_string(&question.text).unwrap_or_default()
    )
}

/// Parses `tool NAME`, `host_tool NAME`, `network DOMAIN[:PORT]` (a space or a colon
/// between the kind and the scope); `host_read` and `host_write` are refused as reserved
/// (a colon may separate the kind and the scope, as on the command line).
pub fn parse_grant(text: &str) -> Result<GrantKind, &'static str> {
    let text = text.trim();
    let (kind, rest) = text
        .split_once([' ', ':'])
        .map(|(k, r)| (k.trim(), r.trim()))
        .ok_or(messages::GRANT_PARSE_ERROR)?;
    if rest.is_empty() {
        return Err(messages::GRANT_PARSE_ERROR);
    }
    Ok(match kind {
        "tool" if rest == mimlys_core::host_tool::RUN_HOST_TOOL => {
            return Err(mimlys_core::host_tool::RUN_HOST_TOOL_NOT_A_GRANT);
        }
        "tool" => GrantKind::Tool {
            name: rest.to_owned(),
        },
        "host_read" | "host_write" => return Err(crate::config::HOST_PATH_GRANTS_RESERVED),
        "network" => {
            let (domain, port) = match rest.rsplit_once(':') {
                Some((d, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (
                    d,
                    Some(p.parse::<u16>().map_err(|_| messages::GRANT_PARSE_ERROR)?),
                ),
                _ => (rest, None),
            };
            GrantKind::Network {
                domains: vec![domain.to_owned()],
                ports: port.into_iter().collect(),
            }
        }
        "host_tool" => GrantKind::HostTool {
            name: rest.to_owned(),
        },
        _ => return Err(messages::GRANT_PARSE_ERROR),
    })
}

/// The terminal: prompts on standard output, reads standard input.
pub struct Terminal<R: BufRead, W: Write> {
    input: R,
    output: W,
    interactive: bool,
}

impl<R: BufRead, W: Write> Terminal<R, W> {
    /// A prompt over the given streams; `interactive` says whether they are a terminal.
    pub fn new(input: R, output: W, interactive: bool) -> Self {
        Self {
            input,
            output,
            interactive,
        }
    }

    fn read_line(&mut self, prompt: &str) -> Option<String> {
        let _ = write!(self.output, "{prompt}");
        let _ = self.output.flush();
        let mut line = String::new();
        match self.input.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_owned()),
        }
    }
}

impl<R: BufRead, W: Write> Interaction for Terminal<R, W> {
    fn interactive(&self) -> bool {
        self.interactive
    }

    fn approve(&mut self, request: &ApprovalRequest, step_index: Option<u32>) -> Decision {
        let _ = write!(self.output, "{}", approval_text(request, step_index));
        loop {
            let Some(line) = self.read_line(messages::PROMPT) else {
                return Decision::Leave;
            };
            match line.trim() {
                "a" => return Decision::Approve,
                "r" | "h" => {
                    let halt = line.trim() == "h";
                    let note = self
                        .read_line(messages::REJECT_NOTE_PROMPT)
                        .filter(|n| !n.trim().is_empty());
                    if note
                        .as_ref()
                        .is_some_and(|n| n.chars().count() > LIM_D2_REASON_CHARS)
                    {
                        let _ = writeln!(self.output, "{}", messages::note_too_long());
                        continue;
                    }
                    return Decision::Reject { note, halt };
                }
                "g" => {
                    let Some(text) = self.read_line(messages::GRANT_PROMPT) else {
                        return Decision::Leave;
                    };
                    match parse_grant(&text) {
                        Ok(kind) => return Decision::Grant(kind),
                        Err(e) => {
                            let _ = writeln!(self.output, "{e}");
                        }
                    }
                }
                _ => {
                    let _ = writeln!(self.output, "{}", messages::PROMPT_UNKNOWN);
                }
            }
        }
    }

    fn answer(&mut self, question: &Question) -> Option<String> {
        let _ = write!(self.output, "{}", question_text(question));
        loop {
            let line = self.read_line(messages::ANSWER_PROMPT)?;
            if line.trim().is_empty() {
                continue;
            }
            return Some(line);
        }
    }

    fn confirm(&mut self, question: &str) -> bool {
        matches!(
            self.read_line(&format!("{question} [y/N] "))
                .as_deref()
                .map(str::trim),
            Some("y") | Some("Y")
        )
    }
}

/// A scripted channel for the test harness: the same decision every time.
pub struct Scripted {
    /// The decision for every approval.
    pub decision: Decision,
    /// The answer to every question.
    pub answer: String,
}

impl Interaction for Scripted {
    fn interactive(&self) -> bool {
        true
    }

    fn approve(&mut self, _request: &ApprovalRequest, _step_index: Option<u32>) -> Decision {
        self.decision.clone()
    }

    fn answer(&mut self, _question: &Question) -> Option<String> {
        Some(self.answer.clone())
    }

    fn confirm(&mut self, _question: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
