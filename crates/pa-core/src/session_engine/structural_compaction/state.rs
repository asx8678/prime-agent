//! The typed state a structural summary renders from. It is saved on every
//! compaction row and folded forward one event at a time, so folding a saved
//! state further gives exactly the state of folding the whole history at once.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::bounds::{one_line, Addressed, Excerpt, Line, Sample};
use super::events::{BashOutcome, CellOutcome, CellStatus, Event, NoticeKind};

/// Bumped whenever the saved shape or a fold rule changes: a state saved by
/// another version is not folded further (the previous summary is carried
/// once as a labeled excerpt instead). Version 2 added `carried`.
pub(crate) const STATE_VERSION: u32 = 2;

const REQUESTS: usize = 24;
const EXCHANGES: usize = 3;
/// One user or assistant text kept for the recent-dialogue block.
pub(super) const EXCHANGE_TEXT_BYTES: usize = 3_968;
const ACTIVITY: usize = 48;
const TURNS: usize = 32;
const TRANSCRIPT: usize = 40;
const BLOCKS: usize = 8;
const BLOCK_BYTES: usize = 1_536;
const EARLIER_SUMMARY_BYTES: usize = 4_096;
const OPEN_FAILURES: usize = 24;
const RESOLVED_FAILURES: usize = 8;
const PENDING_CALLS: usize = 32;
const LINE_CHARS: usize = 140;
const SUBJECT_CHARS: usize = 100;

/// Text and the address it was said at.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Said {
    pub(crate) at: String,
    pub(crate) text: Excerpt,
}

/// A user message and the assistant text it answered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Exchange {
    pub(crate) user: Said,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reply: Option<Said>,
}

/// What failed. Only a later success of the same kind and subject resolves a
/// failure: a cell that runs without an exception says nothing about a shell
/// command's exit code, so a command is resolved only by its own exit 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FailureKind {
    /// A Python cell raised or was aborted.
    Cell,
    /// A shell command exited non-zero, was cancelled, or failed to start.
    Command,
    /// Another tool returned an error.
    Tool,
    /// A subagent failed.
    Child,
}

/// A failed cell, command, tool, or subagent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Failure {
    pub(crate) at: String,
    pub(crate) kind: FailureKind,
    pub(crate) subject: String,
    pub(crate) error: String,
}

/// Where a carried block of model-written text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum BlockOrigin {
    /// A `/tree` branch summary on the active path.
    BranchSummary,
    /// The summary of a compaction made by another engine.
    EarlierSummary,
    /// The summary of a compaction this engine wrote, carried after its
    /// saved state was dropped (a version bump or an unreadable state).
    EarlierDeterministic,
    /// The result of a closed context fold (`fold.end`).
    Fold,
}

/// Model-written text carried verbatim (bounded), never summarized again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Block {
    pub(crate) at: String,
    pub(crate) origin: BlockOrigin,
    pub(crate) text: Excerpt,
}

impl Addressed for Block {
    fn address(&self) -> &str {
        &self.at
    }
}

/// A tool call waiting for its result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Call {
    id: String,
    at: String,
    subject: String,
}

/// The counters of the turn being folded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Turn {
    pub(crate) at: String,
    pub(crate) label: String,
    pub(crate) cells: u64,
    pub(crate) bash: u64,
    pub(crate) edits: u64,
    pub(crate) failures: u64,
}

impl Turn {
    #[must_use]
    pub(crate) fn line(&self) -> Line {
        let failed = if self.failures > 0 {
            format!(" failed:{}", self.failures)
        } else {
            String::new()
        };
        Line::new(
            &self.at,
            format!(
                "{} | cells:{} bash:{} edits:{}{failed}",
                self.label, self.cells, self.bash, self.edits
            ),
        )
    }
}

/// The latest facts the current-status block names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Status {
    pub(crate) request: Option<Line>,
    pub(crate) change: Option<Line>,
    pub(crate) action: Option<Line>,
    pub(crate) note: Option<Line>,
}

/// Everything a structural summary is rendered from. Addresses are positions
/// on the branch (`u12` is the 12th user message, `u12.c3` the third Python
/// cell after it), so they mean the same thing in every copy of the log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct State {
    pub(crate) version: u32,
    pub(super) users: u64,
    pub(super) cells: u64,
    pub(super) events: u64,
    pub(super) thinking: u64,
    pub(super) requests: Sample<Line>,
    pub(super) exchanges: VecDeque<Exchange>,
    pub(super) reply: Option<Said>,
    calls: VecDeque<Call>,
    pub(super) open_failures: Vec<Failure>,
    pub(super) open_failures_omitted: u64,
    pub(super) resolved_failures: VecDeque<Failure>,
    pub(super) resolved_failures_omitted: u64,
    pub(super) activity: Sample<Line>,
    pub(super) turns: Sample<Line>,
    pub(super) turn: Option<Turn>,
    pub(super) transcript: VecDeque<Line>,
    pub(super) transcript_omitted: u64,
    pub(super) status: Status,
    pub(super) blocks: Sample<Block>,
    /// The compaction carry-forward list: the latest carry row's items,
    /// rendered into every summary until replaced or cleared.
    #[serde(default)]
    pub(super) carried: Vec<String>,
}

impl State {
    #[must_use]
    pub(crate) fn new() -> State {
        State {
            version: STATE_VERSION,
            ..State::default()
        }
    }

    /// A fresh state carrying a bounded, labeled excerpt of a summary
    /// another engine wrote, so switching engines keeps that context once.
    /// A summary this engine wrote (its saved state was dropped) keeps its
    /// deterministic label.
    #[must_use]
    pub(crate) fn from_summary(summary: &str) -> State {
        let mut state = State::new();
        if !summary.trim().is_empty() {
            let origin = if summary.starts_with(super::render::HEADER) {
                BlockOrigin::EarlierDeterministic
            } else {
                BlockOrigin::EarlierSummary
            };
            let at = state.address();
            state.blocks.push(
                Block {
                    at,
                    origin,
                    text: Excerpt::of(summary, EARLIER_SUMMARY_BYTES),
                },
                BLOCKS,
            );
        }
        state
    }

    /// The state saved on a compaction row, when this version saved it.
    #[must_use]
    pub(crate) fn from_value(value: &Value) -> Option<State> {
        let state: State = serde_json::from_value(value.clone()).ok()?;
        (state.version == STATE_VERSION).then_some(state)
    }

    /// The form saved on a compaction row. A serialization failure is an error, never a
    /// silently saved `null` (which the next compaction would read as "no state").
    pub(crate) fn to_value(&self) -> serde_json::Result<Value> {
        serde_json::to_value(self)
    }

    /// Fold `events` in log order.
    pub(crate) fn fold(&mut self, events: &[Event]) {
        for event in events {
            self.apply(event);
        }
    }

    /// The current address: `uN`, or `uN.cM` once a cell ran after it.
    #[must_use]
    pub(super) fn address(&self) -> String {
        if self.cells == 0 {
            format!("u{}", self.users)
        } else {
            format!("u{}.c{}", self.users, self.cells)
        }
    }

    fn apply(&mut self, event: &Event) {
        self.events += 1;
        match event {
            Event::User { text } => self.user(text),
            Event::AssistantText { text } => self.assistant(text),
            Event::Thinking => self.thinking += 1,
            Event::CellStart { call_id, preview } => self.cell_start(call_id, preview),
            Event::CellEnd { call_id, outcome } => self.cell_end(call_id, outcome),
            Event::ToolStart {
                call_id,
                name,
                subject,
            } => {
                let at = self.address();
                let subject = if subject.is_empty() {
                    name.clone()
                } else {
                    format!("{name}: {}", one_line(subject, SUBJECT_CHARS))
                };
                self.wait(call_id, &at, subject);
            }
            Event::ToolEnd { call_id, error } => {
                let (at, subject) = self.answer(call_id, "tool");
                let error = error
                    .as_deref()
                    .map(|error| non_empty(one_line(error, LINE_CHARS), "error"));
                self.settle(&at, FailureKind::Tool, subject, error);
            }
            Event::BashDone { command, outcome } => {
                let at = self.address();
                let subject = format!("bash: {}", one_line(command, SUBJECT_CHARS));
                let error = match outcome {
                    BashOutcome::Exited(0) => None,
                    BashOutcome::Exited(code) => Some(format!("exit {code}")),
                    BashOutcome::Cancelled => Some("cancelled".to_string()),
                    // Neutral for both a spawn failure and a signal death:
                    // the row records no exit code either way.
                    BashOutcome::NoExitCode => Some("ended without an exit code".to_string()),
                };
                self.settle(&at, FailureKind::Command, subject, error);
            }
            Event::MessageIn {
                from,
                relationship,
                text,
            } => self.message_in(from, relationship.as_deref(), text),
            Event::ChildEnded { name, kind, detail } => {
                self.child_ended(name, kind, detail.as_deref());
            }
            Event::Notice { kind, text } => self.notice(*kind, text),
            Event::CarryItems { items } => self.carried.clone_from(items),
            Event::Custom { custom_type, text } => {
                let at = self.address();
                self.note(
                    &at,
                    format!("{custom_type}: {}", one_line(text, LINE_CHARS)),
                );
            }
            Event::BranchSummary { text } => {
                let at = self.address();
                self.blocks.push(
                    Block {
                        at: at.clone(),
                        origin: BlockOrigin::BranchSummary,
                        text: Excerpt::of(text, BLOCK_BYTES),
                    },
                    BLOCKS,
                );
                self.note(&at, "branch summary".to_string());
            }
            Event::FoldClosed {
                description,
                result,
            } => {
                let at = self.address();
                self.blocks.push(
                    Block {
                        at: at.clone(),
                        origin: BlockOrigin::Fold,
                        text: Excerpt::of(&format!("{description}: {result}"), BLOCK_BYTES),
                    },
                    BLOCKS,
                );
                self.note(
                    &at,
                    format!("fold closed: {}", one_line(description, SUBJECT_CHARS)),
                );
            }
        }
    }

    fn user(&mut self, text: &str) {
        self.users += 1;
        self.cells = 0;
        let at = self.address();
        let line = non_empty(one_line(text, LINE_CHARS), "(no text)");
        self.start_turn(format!("\"{line}\""));
        self.requests.push(Line::new(&at, line.clone()), REQUESTS);
        self.exchanges.push_back(Exchange {
            user: Said {
                at: at.clone(),
                text: Excerpt::of(text, EXCHANGE_TEXT_BYTES),
            },
            reply: self.reply.take(),
        });
        if self.exchanges.len() > EXCHANGES {
            self.exchanges.pop_front();
        }
        self.status.request = Some(Line::new(&at, line.clone()));
        self.note(&at, format!("user: {line}"));
    }

    fn assistant(&mut self, text: &str) {
        let at = self.address();
        let line = one_line(text, LINE_CHARS);
        self.reply = Some(Said {
            at: at.clone(),
            text: Excerpt::of(text, EXCHANGE_TEXT_BYTES),
        });
        self.status.note = Some(Line::new(&at, line.clone()));
        self.note(&at, format!("assistant: {line}"));
    }

    fn cell_start(&mut self, call_id: &str, preview: &str) {
        self.cells += 1;
        let at = self.address();
        let subject = format!("python: {}", one_line(preview, SUBJECT_CHARS));
        self.turn_mut().cells += 1;
        self.wait(call_id, &at, subject);
    }

    fn cell_end(&mut self, call_id: &str, outcome: &CellOutcome) {
        let (at, subject) = self.answer(call_id, "python");
        let subject = outcome.bash_first.as_deref().map_or(subject, |command| {
            format!("bash: {}", one_line(command, SUBJECT_CHARS))
        });
        let edits = u64::try_from(outcome.edited.len()).unwrap_or(u64::MAX);
        let turn = self.turn_mut();
        turn.bash += outcome.bash_count;
        turn.edits += edits;
        if let Some(path) = outcome.edited.last() {
            self.status.change = Some(Line::new(&at, path.clone()));
        }
        for sent in &outcome.sent {
            let delivery = if sent.queued { "queued" } else { "delivered" };
            self.activity.push(
                Line::new(&at, format!("message \u{2192} {} ({delivery})", sent.to)),
                ACTIVITY,
            );
        }
        let error = match outcome.status {
            CellStatus::Ok => None,
            CellStatus::Error => Some(outcome.error.clone().unwrap_or_else(|| "error".to_string())),
            CellStatus::Aborted => Some("aborted".to_string()),
        };
        self.settle(&at, FailureKind::Cell, subject, error);
    }

    fn message_in(&mut self, from: &str, relationship: Option<&str>, text: &str) {
        let at = self.address();
        let relationship = relationship.map(|relationship| format!(" ({relationship})"));
        let relationship = relationship.unwrap_or_default();
        let line = one_line(text, LINE_CHARS);
        self.start_turn(format!("message \u{2190} {from}{relationship}"));
        self.activity.push(
            Line::new(
                &at,
                format!("message \u{2190} {from}{relationship}: {line}"),
            ),
            ACTIVITY,
        );
        self.note(&at, format!("message \u{2190} {from}: {line}"));
    }

    fn child_ended(&mut self, name: &str, kind: &str, detail: Option<&str>) {
        let at = self.address();
        let detail = detail
            .map(|detail| one_line(detail, LINE_CHARS))
            .filter(|detail| !detail.is_empty());
        let text = match &detail {
            Some(detail) => format!("child {name}: {kind} \u{2014} {detail}"),
            None => format!("child {name}: {kind}"),
        };
        self.activity.push(Line::new(&at, text.clone()), ACTIVITY);
        self.note(&at, text);
        if kind == "failed" {
            self.turn_mut().failures += 1;
            self.record(Failure {
                at,
                kind: FailureKind::Child,
                subject: format!("child {name}"),
                error: detail.unwrap_or_else(|| "failed".to_string()),
            });
        }
    }

    /// Prompts that recur (heartbeats, goal and autonomous continuations)
    /// start turns and stay out of the activity sample, so repeats never
    /// crowd out agent interactions; the others are activity.
    fn notice(&mut self, kind: NoticeKind, text: &str) {
        let at = self.address();
        let first_line = one_line(text, LINE_CHARS);
        let (line, starts_turn) = match kind {
            NoticeKind::Heartbeat => (format!("heartbeat: {first_line}"), true),
            NoticeKind::Goal => ("goal continuation".to_string(), true),
            NoticeKind::Continuation => (first_line, true),
            NoticeKind::Refinement => (format!("refinement: {first_line}"), false),
            NoticeKind::Autonomous => (format!("autonomous: {first_line}"), false),
        };
        if starts_turn {
            self.start_turn(line.clone());
        } else {
            self.activity.push(Line::new(&at, line.clone()), ACTIVITY);
        }
        self.note(&at, line);
    }

    /// Close the open turn and open one labeled `label` at the current address.
    fn start_turn(&mut self, label: String) {
        if let Some(turn) = self.turn.take() {
            self.turns.push(turn.line(), TURNS);
        }
        self.turn = Some(Turn {
            at: self.address(),
            label,
            ..Turn::default()
        });
    }

    fn turn_mut(&mut self) -> &mut Turn {
        let at = self.address();
        self.turn.get_or_insert_with(|| Turn {
            at,
            label: "(start)".to_string(),
            ..Turn::default()
        })
    }

    /// Add one transcript line, keeping the latest [`TRANSCRIPT`].
    fn note(&mut self, at: &str, text: String) {
        self.transcript.push_back(Line::new(at, text));
        if self.transcript.len() > TRANSCRIPT {
            self.transcript.pop_front();
            self.transcript_omitted += 1;
        }
    }

    fn wait(&mut self, call_id: &str, at: &str, subject: String) {
        self.calls.push_back(Call {
            id: call_id.to_string(),
            at: at.to_string(),
            subject,
        });
        if self.calls.len() > PENDING_CALLS {
            self.calls.pop_front();
        }
    }

    /// The address and subject of the call `call_id` answers.
    fn answer(&mut self, call_id: &str, fallback: &str) -> (String, String) {
        let index = self.calls.iter().position(|call| call.id == call_id);
        match index.and_then(|index| self.calls.remove(index)) {
            Some(call) => (call.at, call.subject),
            None => (self.address(), fallback.to_string()),
        }
    }

    /// Record an action's outcome as one transcript line: a success resolves
    /// earlier failures of the same kind and subject, an error opens one.
    fn settle(&mut self, at: &str, kind: FailureKind, subject: String, error: Option<String>) {
        let line = format!(
            "{subject}{} \u{2192} {}",
            match kind {
                FailureKind::Command => " finished",
                FailureKind::Cell | FailureKind::Tool | FailureKind::Child => "",
            },
            error.as_deref().unwrap_or("ok")
        );
        self.status.action = Some(Line::new(at, line.clone()));
        self.note(at, line);
        match error {
            None => self.resolve(kind, &subject),
            Some(error) => {
                self.turn_mut().failures += 1;
                self.record(Failure {
                    at: at.to_string(),
                    kind,
                    subject,
                    error,
                });
            }
        }
    }

    /// Open a failure; when full, the oldest of the later half drops out, so
    /// the earliest and the latest stay visible.
    fn record(&mut self, failure: Failure) {
        if self.open_failures.len() >= OPEN_FAILURES {
            self.open_failures.remove(OPEN_FAILURES / 2);
            self.open_failures_omitted += 1;
        }
        self.open_failures.push(failure);
    }

    fn resolve(&mut self, kind: FailureKind, subject: &str) {
        let (resolved, open): (Vec<Failure>, Vec<Failure>) =
            std::mem::take(&mut self.open_failures)
                .into_iter()
                .partition(|failure| failure.kind == kind && failure.subject == subject);
        self.open_failures = open;
        for failure in resolved {
            self.resolved_failures.push_back(failure);
            if self.resolved_failures.len() > RESOLVED_FAILURES {
                self.resolved_failures.pop_front();
                self.resolved_failures_omitted += 1;
            }
        }
    }
}

fn non_empty(text: String, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}
