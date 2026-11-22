//! The approval and question prompts (external design §10.1; REQ-CLI-002,
//! REQ-APR-009). The prompt prints the runtime's rendered request through a
//! fixed template and nothing else from the task; it reads one of `a`, `r`,
//! `h`, `g`. The same `Interaction` trait lets the test harness answer
//! without a terminal (the fake approval channel of REQ-TEST-002). The wait
//! for the user can be ended by a stop signal (REQ-CLI-005): standard input
//! is read on a thread of its own and the prompt waits on a queue that the
//! stop signal is put to as well.

use crate::messages;
use mimlys_core::approval::{ApprovalRequest, arguments_text};
use mimlys_core::grant::GrantKind;
use mimlys_core::limits::LIM_D2_REASON_CHARS;
use mimlys_core::question::Question;
use std::io::{BufRead, Write};
use std::sync::mpsc::{Receiver, Sender};

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
    /// The handle that ends a wait of this channel for the user, when it has one
    /// that can be ended. A stop signal that arrives while the user is asked
    /// puts the stop to it (REQ-CLI-005); a channel that answers at once has none.
    fn stop_handle(&self) -> Option<StopHandle> {
        None
    }
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

/// Every argument of the request in full, sanitized, one per line, none cut
/// (REQ-APR-002): what `[v]iew` and `task show --arguments` print.
pub fn arguments_in_full(request: &ApprovalRequest) -> String {
    let mut text = String::from(messages::ARGUMENTS_IN_FULL);
    match &request.arguments {
        Some(arguments) => {
            for line in arguments_text(arguments).lines() {
                text.push_str("\n  ");
                text.push_str(line);
            }
        }
        None => {
            text.push_str("\n  ");
            text.push_str(messages::ARGUMENTS_NOT_AVAILABLE);
        }
    }
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

/// What one wait for the user's input ended with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// A line, without its line ending.
    Line(String),
    /// The input ended.
    End,
    /// A stop signal arrived while the prompt waited (REQ-CLI-005).
    Stop,
}

/// Gives up a wait for input: a stop signal arrived. Cheap to clone and to
/// send to the task that listens for the signals.
#[derive(Clone, Debug)]
pub struct StopHandle {
    tx: Sender<Input>,
}

impl StopHandle {
    /// Ends the wait of the prompt that holds the other end, now or at its
    /// next wait.
    pub fn stop(&self) {
        let _ = self.tx.send(Input::Stop);
    }
}

/// Where a prompt gets its lines. The wait for one can be ended by a
/// [`StopHandle`], so that a stop signal is not held up by a read that never
/// comes.
pub trait Lines {
    /// Waits for the next line, the end of the input, or a stop.
    fn next(&mut self) -> Input;
    /// The handle that ends the wait, when this source can be stopped.
    fn stop_handle(&self) -> Option<StopHandle> {
        None
    }
}

/// Lines read in place from a buffered reader: a wait that nothing stops. For
/// tests and for a reader that does not block.
pub struct Buffered<R>(R);

impl<R: BufRead> Lines for Buffered<R> {
    fn next(&mut self) -> Input {
        let mut line = String::new();
        match self.0.read_line(&mut line) {
            Ok(0) | Err(_) => Input::End,
            Ok(_) => Input::Line(line.trim_end_matches(['\r', '\n']).to_owned()),
        }
    }
}

/// A queue of inputs that a wait takes from, and that anything holding a
/// sender can put to: the lines a reader found, and the stop.
pub struct Queue {
    tx: Sender<Input>,
    rx: Receiver<Input>,
}

impl Queue {
    /// An empty queue.
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { tx, rx }
    }

    /// The next input, or `None` when none comes within `bound`: for tests, so
    /// that a wait that is not ended fails instead of hanging.
    pub fn next_within(&mut self, bound: std::time::Duration) -> Option<Input> {
        self.rx.recv_timeout(bound).ok()
    }

    /// A sender to put inputs to this queue.
    pub fn sender(&self) -> Sender<Input> {
        self.tx.clone()
    }
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Lines for Queue {
    fn next(&mut self) -> Input {
        self.rx.recv().unwrap_or(Input::End)
    }

    fn stop_handle(&self) -> Option<StopHandle> {
        Some(StopHandle {
            tx: self.tx.clone(),
        })
    }
}

/// Standard input, read on a thread of its own so that the wait for a line
/// races the stop signals (REQ-CLI-005).
///
/// A read of a terminal blocks until the user types, and nothing can interrupt
/// it from outside. So the read runs on a detached thread that puts what it
/// found on a [`Queue`], and the prompt waits on the queue, where a stop is put
/// too: whichever comes first ends the wait. At most one read is outstanding.
/// After a stop the thread may still be blocked in its read; it holds nothing
/// the process needs and it is not joined, so it does not keep the process
/// alive: the process ends when `main` returns, whatever threads remain.
pub struct StdinLines {
    queue: Queue,
    reading: bool,
}

impl StdinLines {
    /// A reader of standard input; no thread is started before the first wait.
    pub fn new() -> Self {
        Self {
            queue: Queue::new(),
            reading: false,
        }
    }

    fn start_read(&mut self) -> bool {
        let tx = self.queue.sender();
        let spawned = std::thread::Builder::new()
            .name("mimlys-stdin".into())
            .spawn(move || {
                let mut line = String::new();
                let input = match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => Input::End,
                    Ok(_) => Input::Line(line.trim_end_matches(['\r', '\n']).to_owned()),
                };
                let _ = tx.send(input);
            });
        spawned.is_ok()
    }
}

impl Default for StdinLines {
    fn default() -> Self {
        Self::new()
    }
}

impl Lines for StdinLines {
    fn next(&mut self) -> Input {
        if !self.reading {
            if !self.start_read() {
                return Input::End;
            }
            self.reading = true;
        }
        let input = self.queue.next();
        if !matches!(input, Input::Stop) {
            // The outstanding read has answered; after a stop it has not.
            self.reading = false;
        }
        input
    }

    fn stop_handle(&self) -> Option<StopHandle> {
        self.queue.stop_handle()
    }
}

/// The terminal: prompts on standard output, reads standard input.
pub struct Terminal<L: Lines, W: Write> {
    input: L,
    output: W,
    interactive: bool,
    /// A stop ended a wait: nothing is waited for any more.
    stopped: bool,
}

impl<R: BufRead, W: Write> Terminal<Buffered<R>, W> {
    /// A prompt over the given streams; `interactive` says whether they are a terminal.
    pub fn new(input: R, output: W, interactive: bool) -> Self {
        Self::with_lines(Buffered(input), output, interactive)
    }
}

impl Terminal<StdinLines, std::io::Stdout> {
    /// The command line's prompt: standard input, read on a thread of its own, and standard
    /// output. Every write of a prompt ignores its error (`let _ = write!`), so a closed standard
    /// output is no panic here. With the output module, this is the one place that takes the
    /// handle to standard output (`clippy.toml`).
    #[allow(clippy::disallowed_methods)]
    pub fn on_standard_streams(interactive: bool) -> Self {
        Self::with_lines(StdinLines::new(), std::io::stdout(), interactive)
    }
}

impl<L: Lines, W: Write> Terminal<L, W> {
    /// A prompt that gets its lines from `input`.
    pub fn with_lines(input: L, output: W, interactive: bool) -> Self {
        Self {
            input,
            output,
            interactive,
            stopped: false,
        }
    }

    fn read_line(&mut self, prompt: &str) -> Option<String> {
        if self.stopped {
            return None;
        }
        let _ = write!(self.output, "{prompt}");
        let _ = self.output.flush();
        match self.input.next() {
            Input::Line(line) => Some(line),
            Input::End => None,
            Input::Stop => {
                self.stopped = true;
                None
            }
        }
    }
}

impl<L: Lines, W: Write> Interaction for Terminal<L, W> {
    fn interactive(&self) -> bool {
        self.interactive
    }

    fn stop_handle(&self) -> Option<StopHandle> {
        self.input.stop_handle()
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
                    if self.stopped {
                        // The wait for the note ended with a stop: nothing is decided.
                        return Decision::Leave;
                    }
                    if note
                        .as_ref()
                        .is_some_and(|n| n.chars().count() > LIM_D2_REASON_CHARS)
                    {
                        let _ = writeln!(self.output, "{}", messages::note_too_long());
                        continue;
                    }
                    return Decision::Reject { note, halt };
                }
                "v" => {
                    let _ = writeln!(self.output, "{}", arguments_in_full(request));
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
