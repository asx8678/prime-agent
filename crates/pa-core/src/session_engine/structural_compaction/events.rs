//! Session entries to typed events. Only structure is read: message roles,
//! tool names, typed `details` fields, and custom-row types; prose is carried
//! as text and never interpreted.

use pa_types::ai::{AssistantContentBlock, ToolCall, ToolResultMessage, UserContent};
use pa_types::session::{AgentMessage, FileEntry};
use pa_types::slash_commands::{
    SESSION_SLASH_COMMAND_CUSTOM_TYPE, SESSION_SLASH_COMMAND_RESULT_CUSTOM_TYPE,
};
use serde_json::Value;

use crate::autonomous::{AUTONOMOUS_CONTINUATION_LABEL, AUTONOMOUS_STATUS_CUSTOM_TYPE};
use crate::goals::GOAL_CONTEXT_CUSTOM_TYPE;
use crate::prompts::model_prompts::MODEL_PROMPT_ERROR_CUSTOM_TYPE;
use crate::session_engine::agent_messaging::AGENT_MESSAGE_CUSTOM_TYPE;
use crate::session_engine::headless::HARNESS_DIGEST_CUSTOM_TYPE;
use crate::session_engine::ipython_state::IPYTHON_STATE_CUSTOM_TYPE;
use crate::session_engine::messages::{
    ASYNC_BASH_COMPLETION_CUSTOM_TYPE, COMPACTION_OUTCOME_CUSTOM_TYPE,
    HEARTBEAT_PROMPT_CUSTOM_TYPE, PROVIDER_RETRY_OUTCOME_CUSTOM_TYPE,
    REFINEMENT_NOTICE_CUSTOM_TYPE, REFINEMENT_OUTCOME_CUSTOM_TYPE,
};
use crate::session_engine::rlm_notices::{
    RLM_CHILD_FAILURE_CUSTOM_TYPE, RLM_CHILD_TERMINAL_NOTICE_CUSTOM_TYPE,
};
use crate::session_engine::skills_unavailable_notice::PYTHON_SKILLS_UNAVAILABLE_CUSTOM_TYPE;
use crate::session_engine::state_restore_notice::IPYTHON_STATE_RESTORED_CUSTOM_TYPE;

/// The tool whose calls are Python cells.
const IPYTHON_TOOL: &str = "ipython";

/// One typed fact read from a session entry, in log order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// A user message.
    User { text: String },
    /// The joined text blocks of one assistant message.
    AssistantText { text: String },
    /// An assistant thinking block: counted, never carried.
    Thinking,
    /// An `ipython` call, with the cell's first top-level statement (comments,
    /// decorators, imports, `def`/`class` headers, and indented bodies
    /// skipped). One pass over the code: the TUI's scored preview costs far
    /// too much across a compaction window.
    CellStart { call_id: String, preview: String },
    /// An `ipython` result.
    CellEnd {
        call_id: String,
        outcome: CellOutcome,
    },
    /// Any other tool call, with its primary argument.
    ToolStart {
        call_id: String,
        name: String,
        subject: String,
    },
    /// Any other tool result; `error` is the first line of a failed result.
    ToolEnd {
        call_id: String,
        error: Option<String>,
    },
    /// A shell command that finished outside a cell: a user `!` command or a
    /// detached `bash()` completion. `None` when it was cancelled.
    BashDone {
        command: String,
        exit_code: Option<i64>,
    },
    /// A message delivered from another agent.
    MessageIn {
        from: String,
        relationship: Option<String>,
        text: String,
    },
    /// A subagent that ended without replying, was cancelled, or failed.
    ChildEnded {
        name: String,
        kind: String,
        detail: Option<String>,
    },
    /// A harness-injected prompt.
    Notice { kind: NoticeKind, text: String },
    /// Any other model-visible custom row.
    Custom { custom_type: String, text: String },
    /// A summary written once at a branch boundary (`/tree`).
    BranchSummary { text: String },
}

/// The prompts the harness injects into a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeKind {
    /// A heartbeat fire; starts a turn.
    Heartbeat,
    /// A goal continuation; starts a turn.
    Goal,
    /// An autonomous-mode continuation, sent as a user row; starts a turn.
    Continuation,
    /// A harness refinement result.
    Refinement,
    /// An autonomous-mode status.
    Autonomous,
}

/// The typed outcome of one Python cell (`details` of an `ipython` result).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CellOutcome {
    pub(crate) status: CellStatus,
    /// `Ename: first line of evalue`.
    pub(crate) error: Option<String>,
    /// The first `bash()` command the cell started, and how many it started.
    pub(crate) bash_first: Option<String>,
    pub(crate) bash_count: u64,
    /// Files the cell edited, in order.
    pub(crate) edited: Vec<String>,
    /// Agent messages the cell sent.
    pub(crate) sent: Vec<SentMessage>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum CellStatus {
    #[default]
    Ok,
    Error,
    Aborted,
}

/// One agent message sent from a cell: the receiver's display name and
/// whether delivery was queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SentMessage {
    pub(crate) to: String,
    pub(crate) queued: bool,
}

/// Typed events of `entries` in order. Entries the model never reads
/// (state rows, bookkeeping custom rows) produce none, and prior compactions
/// are never source: a new summary folds typed state, not earlier prose.
#[must_use]
pub(crate) fn normalize(entries: &[FileEntry]) -> Vec<Event> {
    let mut events = Vec::new();
    for entry in entries {
        match entry {
            FileEntry::Message { message, .. } => push_message(message, &mut events),
            FileEntry::CustomMessage { payload, .. } => push_custom(
                &payload.custom_type,
                &payload.content,
                payload.details.as_ref(),
                &mut events,
            ),
            FileEntry::BranchSummary { payload, .. } => events.push(Event::BranchSummary {
                text: payload.summary.clone(),
            }),
            FileEntry::Header { .. }
            | FileEntry::ThinkingLevelChange { .. }
            | FileEntry::ServiceTierChange { .. }
            | FileEntry::ModelChange { .. }
            | FileEntry::Compaction { .. }
            | FileEntry::Custom { .. }
            | FileEntry::ChildUsageAttributed { .. }
            | FileEntry::Label { .. }
            | FileEntry::SessionInfo { .. }
            | FileEntry::SessionState { .. }
            | FileEntry::GitState { .. }
            | FileEntry::Unknown { .. } => {}
        }
    }
    events
}

fn push_message(message: &AgentMessage, events: &mut Vec<Event>) {
    match message {
        AgentMessage::User(user) => {
            let text = user.content.text();
            events.push(if text.starts_with(AUTONOMOUS_CONTINUATION_LABEL) {
                notice(NoticeKind::Continuation, text)
            } else {
                Event::User { text }
            });
        }
        AgentMessage::Assistant(assistant) => push_assistant(&assistant.content, events),
        AgentMessage::ToolResult(result) => events.push(tool_end(result)),
        AgentMessage::BashExecution(bash) => {
            if bash.exclude_from_context != Some(true) {
                events.push(Event::BashDone {
                    command: bash.command.clone(),
                    exit_code: if bash.cancelled { None } else { bash.exit_code },
                });
            }
        }
        AgentMessage::Custom(custom) => push_custom(
            &custom.custom_type,
            &custom.content,
            custom.details.as_ref(),
            events,
        ),
        AgentMessage::BranchSummary(summary) => events.push(Event::BranchSummary {
            text: summary.summary.clone(),
        }),
        AgentMessage::CompactionSummary(_) => {}
    }
}

fn push_assistant(content: &[AssistantContentBlock], events: &mut Vec<Event>) {
    let text = content
        .iter()
        .filter_map(|block| match block {
            AssistantContentBlock::Text(text) => Some(text.text.as_str()),
            AssistantContentBlock::Thinking(_) | AssistantContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !text.trim().is_empty() {
        events.push(Event::AssistantText { text });
    }
    for block in content {
        match block {
            AssistantContentBlock::Thinking(_) => events.push(Event::Thinking),
            AssistantContentBlock::ToolCall(call) => events.push(tool_start(call)),
            AssistantContentBlock::Text(_) => {}
        }
    }
}

fn tool_start(call: &ToolCall) -> Event {
    if call.name == IPYTHON_TOOL {
        let code = call
            .arguments
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let lines = || code.lines().filter(|line| !line.trim().is_empty());
        let skipped = ["#", "@", "import ", "from ", "def ", "async def ", "class "];
        let preview = lines()
            .find(|line| {
                !line.starts_with(char::is_whitespace)
                    && !skipped.iter().any(|prefix| line.starts_with(prefix))
            })
            .or_else(|| lines().next())
            .unwrap_or_default()
            .trim()
            .to_string();
        return Event::CellStart {
            call_id: call.id.clone(),
            preview,
        };
    }
    let subject = ["path", "command", "file_path"]
        .iter()
        .find_map(|key| call.arguments.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    Event::ToolStart {
        call_id: call.id.clone(),
        name: call.name.clone(),
        subject,
    }
}

fn tool_end(result: &ToolResultMessage) -> Event {
    if result.tool_name == IPYTHON_TOOL {
        return Event::CellEnd {
            call_id: result.tool_call_id.clone(),
            outcome: cell_outcome(result.details.as_ref(), result.is_error),
        };
    }
    Event::ToolEnd {
        call_id: result.tool_call_id.clone(),
        error: result.is_error.then(|| {
            UserContent::Blocks(result.content.clone())
                .text()
                .lines()
                .next()
                .unwrap_or_default()
                .to_string()
        }),
    }
}

fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn cell_outcome(details: Option<&Value>, is_error: bool) -> CellOutcome {
    let Some(details) = details.filter(|details| details.is_object()) else {
        return CellOutcome {
            status: if is_error {
                CellStatus::Error
            } else {
                CellStatus::Ok
            },
            ..CellOutcome::default()
        };
    };
    let status = match str_at(details, "status") {
        Some("error") => CellStatus::Error,
        Some("aborted") => CellStatus::Aborted,
        Some(_) | None if is_error => CellStatus::Error,
        Some(_) | None => CellStatus::Ok,
    };
    let error = details.get("error").and_then(|error| {
        let ename = str_at(error, "ename").filter(|ename| !ename.is_empty())?;
        let evalue = str_at(error, "evalue")
            .and_then(|evalue| evalue.lines().next())
            .unwrap_or_default();
        Some(if evalue.is_empty() {
            ename.to_string()
        } else {
            format!("{ename}: {evalue}")
        })
    });
    let bash = details.get("bashCommands");
    let mut edited: Vec<String> = Vec::new();
    for path in details
        .get("diffs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|diff| str_at(diff, "path"))
    {
        if !edited.iter().any(|seen| seen == path) {
            edited.push(path.to_string());
        }
    }
    let sent = details
        .get("sentAgentMessages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|sent| SentMessage {
            to: sent
                .get("target")
                .map_or_else(|| "unknown".to_string(), agent_name),
            queued: str_at(sent, "deliveryStatus") == Some("queued"),
        })
        .collect();
    CellOutcome {
        status,
        error,
        bash_first: bash
            .and_then(|bash| str_at(bash, "first"))
            .map(str::to_string),
        bash_count: bash
            .and_then(|bash| bash.get("count"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        edited,
        sent,
    }
}

/// An agent endpoint's display name: session name, then active session id,
/// then session id.
fn agent_name(endpoint: &Value) -> String {
    ["sessionName", "activeSessionId", "sessionId"]
        .iter()
        .find_map(|key| str_at(endpoint, key).filter(|name| !name.is_empty()))
        .unwrap_or("unknown")
        .to_string()
}

fn push_custom(
    custom_type: &str,
    content: &UserContent,
    details: Option<&Value>,
    events: &mut Vec<Event>,
) {
    let null = Value::Null;
    let details = details.unwrap_or(&null);
    let text = content.text();
    let event = match custom_type {
        AGENT_MESSAGE_CUSTOM_TYPE => Event::MessageIn {
            from: details
                .get("from")
                .map_or_else(|| "unknown".to_string(), agent_name),
            relationship: str_at(details, "fromRelationship").map(str::to_string),
            text: str_at(details, "message").map_or(text, str::to_string),
        },
        ASYNC_BASH_COMPLETION_CUSTOM_TYPE => Event::BashDone {
            command: str_at(details, "command").unwrap_or_default().to_string(),
            exit_code: details.get("exitCode").and_then(Value::as_i64),
        },
        RLM_CHILD_TERMINAL_NOTICE_CUSTOM_TYPE => Event::ChildEnded {
            name: child_name(details),
            kind: str_at(details, "kind").unwrap_or("ended").to_string(),
            detail: str_at(details, "reason")
                .or_else(|| str_at(details, "lastAssistantTextPreview"))
                .map(str::to_string),
        },
        RLM_CHILD_FAILURE_CUSTOM_TYPE => Event::ChildEnded {
            name: child_name(details),
            kind: "failed".to_string(),
            detail: str_at(details, "error").map(str::to_string),
        },
        HEARTBEAT_PROMPT_CUSTOM_TYPE => notice(NoticeKind::Heartbeat, text),
        GOAL_CONTEXT_CUSTOM_TYPE => notice(NoticeKind::Goal, text),
        REFINEMENT_NOTICE_CUSTOM_TYPE => notice(NoticeKind::Refinement, text),
        AUTONOMOUS_STATUS_CUSTOM_TYPE => notice(NoticeKind::Autonomous, text),
        // Re-sent after every compaction, or never shown to the model.
        HARNESS_DIGEST_CUSTOM_TYPE
        | IPYTHON_STATE_CUSTOM_TYPE
        | IPYTHON_STATE_RESTORED_CUSTOM_TYPE
        | PYTHON_SKILLS_UNAVAILABLE_CUSTOM_TYPE
        | SESSION_SLASH_COMMAND_CUSTOM_TYPE
        | SESSION_SLASH_COMMAND_RESULT_CUSTOM_TYPE
        | COMPACTION_OUTCOME_CUSTOM_TYPE
        | REFINEMENT_OUTCOME_CUSTOM_TYPE
        | PROVIDER_RETRY_OUTCOME_CUSTOM_TYPE
        | MODEL_PROMPT_ERROR_CUSTOM_TYPE => return,
        other => Event::Custom {
            custom_type: other.to_string(),
            text,
        },
    };
    events.push(event);
}

fn notice(kind: NoticeKind, text: String) -> Event {
    Event::Notice { kind, text }
}

fn child_name(details: &Value) -> String {
    str_at(details, "sessionName")
        .or_else(|| str_at(details, "childId"))
        .unwrap_or("unknown")
        .to_string()
}
