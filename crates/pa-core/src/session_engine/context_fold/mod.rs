//! Context folding (`contextFolding.enabled`): the model wraps a sub-task
//! in `fold.begin` and `fold.end` cells, and every later request leaves out
//! the work between them, keeping the two cells (the end cell carries the
//! fold's result). The `fold` skill marks both cells through the kernel's
//! context-fold display channel, which the ipython tool saves on the cell's
//! result as `details.contextFold`. The session log is never changed: the
//! transcript, resume and compaction keep the full history, and only what a
//! request sends is folded ([`fold_view`]).
//!
//! The rules (next-steps/05-context-folding.md, after Sun et al.,
//! "Scaling Long-Horizon LLM Agent via Context-Folding"):
//! - a begin pairs with the next end. The work strictly between the begin
//!   cell's assistant group (its message and all of that message's tool
//!   results) and the end cell's assistant message is left out, as whole
//!   groups, so no tool call loses its result;
//! - new input inside an open fold voids it, and nothing is left out: any
//!   row the model reads that is neither its own work nor a goal
//!   continuation nudge. Rows the model never reads stay in place;
//! - an unmatched end, or a begin still open, leaves out nothing;
//! - each tool result inside an open fold carries a reminder line (the
//!   paper's branch-mode prompt), or, once the fold passes its budget, a
//!   line asking the model to close it. The line depends only on earlier
//!   messages, so a result's bytes never change between requests.

use std::collections::HashMap;

use pa_agent::types::{AgentMessage, Message, ToolResultContent};
use serde::Deserialize;
use serde_json::Value;

use crate::goals::{GoalContextDetails, GoalContextKind, GOAL_CONTEXT_CUSTOM_TYPE};

use super::messages::loop_convert_to_llm;

#[cfg(test)]
pub(crate) mod tests;

/// The tool-result details key the ipython tool saves a cell's marker under.
const MARKER_DETAILS_KEY: &str = "contextFold";

/// `contextFolding` as one session runs it: the main thread's requests and
/// its side questions see the same view, so both share the cached prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFolding {
    /// Every request sends the whole conversation (the default).
    Off,
    /// Closed folds are left out; an open fold past `max_fold_tokens`
    /// (estimated) asks the model to close it.
    On { max_fold_tokens: u64 },
}

impl ContextFolding {
    /// The messages one request sends.
    #[must_use]
    pub fn view(self, messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
        match self {
            ContextFolding::Off => messages,
            ContextFolding::On { max_fold_tokens } => fold_view(messages, max_fold_tokens),
        }
    }
}

/// One cell's fold marker (`details.contextFold`), as the `fold` skill
/// emits it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum Marker {
    /// `fold.begin`, with the fold's short label.
    Begin { description: String },
    /// `fold.end`.
    End(EndMarker),
}

/// A `fold.end` cell's marker. The host decides `folded` when `fold.end`
/// runs; the view recomputes the pair either way and never trusts it for
/// what it leaves out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct EndMarker {
    pub(crate) description: String,
    /// What the model keeps in place of the folded work.
    pub(crate) result: String,
    /// Whether the fold closed (`false`: it was voided or refused).
    pub(crate) folded: bool,
    /// How many messages the close hid, and their estimated tokens.
    pub(crate) messages: u64,
    pub(crate) tokens: u64,
    /// Whether the fold had passed `maxFoldTokens` when it closed.
    pub(crate) over_budget: bool,
}

impl Marker {
    /// Decode a tool result's details. Anything malformed is no marker, so
    /// it leaves nothing out.
    pub(crate) fn from_details(details: Option<&Value>) -> Option<Marker> {
        serde_json::from_value(details?.get(MARKER_DETAILS_KEY)?.clone()).ok()
    }
}

/// The end marker of a fold that closed, when `details` carries one.
pub(crate) fn closed_fold(details: Option<&Value>) -> Option<EndMarker> {
    match Marker::from_details(details)? {
        Marker::End(end) => end.folded.then_some(end),
        Marker::Begin { .. } => None,
    }
}

/// How the fold rules read one loop message.
enum Row<'a> {
    /// An assistant message, with the ids of the tool calls it made.
    Assistant(Vec<&'a str>),
    /// A tool result: the call it answers, and the cell's marker.
    ToolResult {
        call_id: &'a str,
        marker: Option<Marker>,
    },
    /// Stays in place and never voids a fold: a row the model never reads
    /// (`loop_convert_to_llm` turns it into nothing), or a goal continuation
    /// nudge (self-contained, and superseded by the next one).
    Kept,
    /// Any other row the model reads: new input, which voids an open fold.
    Input,
}

impl<'a> Row<'a> {
    fn of(message: &'a AgentMessage) -> Row<'a> {
        match message {
            AgentMessage::Standard(Message::Assistant(assistant)) => Row::Assistant(
                assistant
                    .tool_calls()
                    .into_iter()
                    .map(|call| call.id.as_str())
                    .collect(),
            ),
            AgentMessage::Standard(Message::ToolResult(result)) => Row::ToolResult {
                call_id: result.tool_call_id.as_str(),
                marker: Marker::from_details(result.details.as_ref()),
            },
            AgentMessage::Standard(Message::User(_)) => Row::Input,
            AgentMessage::Custom(custom) => {
                let goal_continuation = custom.payload.get("customType").and_then(Value::as_str)
                    == Some(GOAL_CONTEXT_CUSTOM_TYPE)
                    && custom
                        .payload
                        .get("details")
                        .cloned()
                        .and_then(|details| {
                            serde_json::from_value::<GoalContextDetails>(details).ok()
                        })
                        .is_some_and(|details| match details.kind {
                            GoalContextKind::Continuation => true,
                            GoalContextKind::BudgetLimit | GoalContextKind::ObjectiveUpdated => {
                                false
                            }
                        });
                if goal_continuation || loop_convert_to_llm(vec![message.clone()]).is_empty() {
                    Row::Kept
                } else {
                    Row::Input
                }
            }
        }
    }
}

/// The fold being read.
struct OpenFold {
    /// The last index of the begin cell's assistant group.
    after: usize,
    description: String,
    /// The estimated tokens of the work after the begin group so far.
    tokens: u64,
}

/// What the fold rules make of one conversation, per message: whether a
/// request leaves it out, and the line a tool result inside an open fold
/// carries.
struct Folds {
    hidden: Vec<bool>,
    lines: Vec<Option<String>>,
}

impl Folds {
    fn scan(messages: &[AgentMessage], max_fold_tokens: u64) -> Folds {
        let rows: Vec<Row<'_>> = messages.iter().map(Row::of).collect();
        // The assistant message that made each tool call, and the last index
        // of each assistant group (the message and its tool results).
        let mut owner: HashMap<&str, usize> = HashMap::new();
        for (index, row) in rows.iter().enumerate() {
            if let Row::Assistant(calls) = row {
                for call in calls {
                    owner.insert(*call, index);
                }
            }
        }
        let mut group_end: Vec<usize> = (0..rows.len()).collect();
        for (index, row) in rows.iter().enumerate() {
            if let Row::ToolResult { call_id, .. } = row {
                if let Some(&assistant) = owner.get(call_id) {
                    group_end[assistant] = group_end[assistant].max(index);
                }
            }
        }

        let mut folds = Folds {
            hidden: vec![false; rows.len()],
            lines: vec![None; rows.len()],
        };
        let mut open: Option<OpenFold> = None;
        for (index, (row, message)) in rows.iter().zip(messages).enumerate() {
            let inside = open.as_ref().is_some_and(|fold| index > fold.after);
            match row {
                Row::Kept => {}
                Row::Input => {
                    if inside {
                        open = None;
                    }
                }
                Row::Assistant(_) => {
                    if let Some(fold) = open.as_mut().filter(|_| inside) {
                        fold.tokens += estimated_tokens(message);
                    }
                }
                Row::ToolResult { call_id, marker } => {
                    if let Some(fold) = open.as_mut().filter(|_| inside) {
                        fold.tokens += estimated_tokens(message);
                    }
                    match marker {
                        Some(Marker::Begin { description }) => {
                            // A begin inside an open fold is ignored: the
                            // skill refuses nesting, so only a forged marker
                            // gets here.
                            if open.is_none() {
                                let after = owner
                                    .get(call_id)
                                    .map_or(index, |&assistant| group_end[assistant]);
                                open = Some(OpenFold {
                                    after,
                                    description: description.clone(),
                                    tokens: 0,
                                });
                            }
                        }
                        Some(Marker::End(_)) => {
                            // An end in the begin's own group, or one that
                            // answers no known call, closes the fold and
                            // leaves out nothing.
                            if let Some(fold) = open.take() {
                                let end_assistant = owner.get(call_id).copied();
                                if let Some(end_assistant) =
                                    end_assistant.filter(|&assistant| assistant > fold.after)
                                {
                                    let span = fold.after + 1..end_assistant;
                                    for (hidden, row) in
                                        folds.hidden[span.clone()].iter_mut().zip(&rows[span])
                                    {
                                        *hidden = !matches!(row, Row::Kept);
                                    }
                                }
                            }
                        }
                        None => {
                            if let Some(fold) = open.as_ref().filter(|_| inside) {
                                let description = &fold.description;
                                let tokens = fold.tokens;
                                folds.lines[index] = Some(if tokens > max_fold_tokens {
                                    format!(
                                        "[fold over budget: {description}, ~{tokens} tokens] Stop and call await fold.end(result) now, stating the progress made and the work still pending."
                                    )
                                } else {
                                    format!(
                                        "[fold open: {description}] Work only on this fold's goal; when it is done, call await fold.end(result)."
                                    )
                                });
                            }
                        }
                    }
                }
            }
        }
        folds
    }
}

/// The chars/4 estimate compaction uses, over the message's wire JSON.
fn estimated_tokens(message: &AgentMessage) -> u64 {
    pa_types::usage::estimate_tokens(&serde_json::to_value(message).unwrap_or_default())
}

/// The messages one request sends: closed folds left out, and the fold
/// lines on the tool results inside an open fold. A conversation without a
/// fold comes back unchanged.
#[must_use]
fn fold_view(messages: Vec<AgentMessage>, max_fold_tokens: u64) -> Vec<AgentMessage> {
    let any_fold = messages.iter().any(|message| match message {
        AgentMessage::Standard(Message::ToolResult(result)) => matches!(
            Marker::from_details(result.details.as_ref()),
            Some(Marker::Begin { .. })
        ),
        AgentMessage::Standard(Message::User(_) | Message::Assistant(_))
        | AgentMessage::Custom(_) => false,
    });
    if !any_fold {
        return messages;
    }
    let Folds { hidden, lines } = Folds::scan(&messages, max_fold_tokens);
    messages
        .into_iter()
        .zip(hidden)
        .zip(lines)
        .filter(|((_, hidden), _)| !hidden)
        .map(|((mut message, _), line)| {
            if let (AgentMessage::Standard(Message::ToolResult(result)), Some(line)) =
                (&mut message, line)
            {
                result.content.push(ToolResultContent::text(line));
            }
            message
        })
        .collect()
}
