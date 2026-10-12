use pa_agent::types::{
    AgentMessage, AssistantContent, AssistantMessage, Message, StopReason, TextContent, ToolCall,
    ToolResultContent, ToolResultMessage, Usage, UserContent, UserMessage,
};
use serde_json::{json, Value};

use super::fold_view;

/// A generous budget: no reminder turns into the over-budget line.
const BUDGET: u64 = 1_000_000;

pub(crate) fn assistant(calls: &[&str]) -> AgentMessage {
    AgentMessage::Standard(Message::Assistant(AssistantMessage {
        content: calls
            .iter()
            .map(|id| {
                AssistantContent::ToolCall(ToolCall {
                    id: (*id).to_string(),
                    name: "ipython".to_string(),
                    arguments: json!({ "code": format!("# {id}") }),
                    thought_signature: None,
                })
            })
            .collect(),
        api: "test".to_string(),
        provider: "test".to_string(),
        model: "m".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::zero(),
        stop_reason: StopReason::ToolUse,
        stop_reason_raw: None,
        error_message: None,
        timestamp: 1,
    }))
}

fn result_with(call: &str, marker: Option<Value>) -> AgentMessage {
    AgentMessage::Standard(Message::ToolResult(ToolResultMessage {
        tool_call_id: call.to_string(),
        tool_name: "ipython".to_string(),
        content: vec![ToolResultContent::text(format!("output of {call}"))],
        details: Some(match marker {
            Some(marker) => json!({ "status": "ok", "contextFold": marker }),
            None => json!({ "status": "ok" }),
        }),
        is_error: false,
        timestamp: 1,
    }))
}

pub(crate) fn result(call: &str) -> AgentMessage {
    result_with(call, None)
}

pub(crate) fn begin(call: &str) -> AgentMessage {
    result_with(
        call,
        Some(json!({ "kind": "begin", "description": "explore", "goal": "find the bug" })),
    )
}

pub(crate) fn end(call: &str) -> AgentMessage {
    result_with(
        call,
        Some(json!({ "kind": "end", "description": "explore", "result": "found it" })),
    )
}

pub(crate) fn user(text: &str) -> AgentMessage {
    AgentMessage::Standard(Message::User(UserMessage {
        content: UserContent::Parts(vec![pa_agent::types::UserPart::Text(TextContent {
            text: text.to_string(),
            text_signature: None,
        })]),
        timestamp: 1,
    }))
}

/// A loop custom row in its session wire shape.
fn custom(custom_type: &str, display: bool, details: &Value) -> AgentMessage {
    serde_json::from_value(json!({
        "role": "custom",
        "customType": custom_type,
        "content": format!("[{custom_type}]"),
        "display": display,
        "details": details,
        "timestamp": 1,
    }))
    .expect("a loop custom row")
}

fn goal_row(kind: &str) -> AgentMessage {
    custom(
        crate::goals::GOAL_CONTEXT_CUSTOM_TYPE,
        true,
        &json!({
            "kind": kind,
            "objective": "ship it",
            "status": "active",
            "continuationsUsed": 1,
        }),
    )
}

/// The reminder line a tool result inside the open `explore` fold carries.
const REMINDER: &str = "[fold open: explore] Work only on this fold's goal; when it is done, call await fold.end(result).";

/// `message` with `line` appended, as the view sends it.
fn with_line(message: AgentMessage, line: &str) -> AgentMessage {
    let AgentMessage::Standard(Message::ToolResult(mut result)) = message else {
        panic!("only tool results carry fold lines");
    };
    result.content.push(ToolResultContent::text(line));
    AgentMessage::Standard(Message::ToolResult(result))
}

#[test]
fn the_view_is_the_identity_without_markers() {
    let messages = vec![user("go"), assistant(&["c1"]), result("c1"), user("more")];
    assert_eq!(fold_view(messages.clone(), BUDGET), messages);
}

#[test]
fn a_closed_fold_leaves_out_the_work_between_its_begin_and_end_groups() {
    let messages = vec![
        user("go"),
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
        assistant(&["c3"]),
        result("c3"),
        assistant(&["c4"]),
        end("c4"),
    ];
    assert_eq!(
        fold_view(messages, BUDGET),
        vec![
            user("go"),
            assistant(&["c1"]),
            begin("c1"),
            assistant(&["c4"]),
            end("c4"),
        ]
    );
}

#[test]
fn the_prefix_up_to_the_begin_group_is_byte_identical_after_the_fold_closes() {
    let open = vec![
        user("go"),
        assistant(&["c0"]),
        result("c0"),
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
    ];
    let mut closed = open.clone();
    closed.extend([assistant(&["c3"]), end("c3")]);
    let before = serde_json::to_vec(&fold_view(open, BUDGET)[..5]).unwrap();
    let after = serde_json::to_vec(&fold_view(closed, BUDGET)[..5]).unwrap();
    assert_eq!(before, after);
}

#[test]
fn whole_assistant_groups_are_left_out_with_all_their_tool_results() {
    // The begin cell shares its message with a sibling call, and so does the
    // end cell: both groups stay whole, and a two-call group inside the fold
    // goes whole.
    let messages = vec![
        assistant(&["c1", "c1b"]),
        begin("c1"),
        result("c1b"),
        assistant(&["c2", "c2b"]),
        result("c2"),
        result("c2b"),
        assistant(&["c3b", "c3"]),
        result("c3b"),
        end("c3"),
    ];
    assert_eq!(
        fold_view(messages, BUDGET),
        vec![
            assistant(&["c1", "c1b"]),
            begin("c1"),
            result("c1b"),
            assistant(&["c3b", "c3"]),
            with_line(result("c3b"), REMINDER),
            end("c3"),
        ]
    );
}

#[test]
fn new_input_inside_the_span_voids_the_fold() {
    let inputs = [
        ("a user prompt or steering message", user("actually, stop")),
        (
            "a user ! command",
            serde_json::from_value(json!({
                "role": "bashExecution",
                "command": "ls",
                "output": "a.py",
                "exitCode": 0,
                "cancelled": false,
                "truncated": false,
                "timestamp": 1,
            }))
            .unwrap(),
        ),
        (
            "a harness digest",
            custom("harness_digest", false, &json!({ "digest": "d" })),
        ),
        ("a goal budget-limit row", goal_row("budget_limit")),
        (
            "a goal objective-updated row",
            goal_row("objective_updated"),
        ),
    ];
    for (label, input) in inputs {
        let messages = vec![
            assistant(&["c1"]),
            begin("c1"),
            assistant(&["c2"]),
            result("c2"),
            input.clone(),
            assistant(&["c3"]),
            result("c3"),
            assistant(&["c4"]),
            end("c4"),
        ];
        // Nothing is left out, and the results after the input carry no line:
        // the fold stopped being open there.
        assert_eq!(
            fold_view(messages, BUDGET),
            vec![
                assistant(&["c1"]),
                begin("c1"),
                assistant(&["c2"]),
                with_line(result("c2"), REMINDER),
                input,
                assistant(&["c3"]),
                result("c3"),
                assistant(&["c4"]),
                end("c4"),
            ],
            "{label}"
        );
    }
}

#[test]
fn a_goal_continuation_row_inside_the_span_stays_and_does_not_void() {
    let messages = vec![
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
        goal_row("continuation"),
        assistant(&["c3"]),
        result("c3"),
        assistant(&["c4"]),
        end("c4"),
    ];
    assert_eq!(
        fold_view(messages, BUDGET),
        vec![
            assistant(&["c1"]),
            begin("c1"),
            goal_row("continuation"),
            assistant(&["c4"]),
            end("c4"),
        ]
    );
}

#[test]
fn a_row_the_model_never_reads_stays_and_does_not_void() {
    // The provider-retry outcome row is display-only: `loop_convert_to_llm`
    // turns it into nothing.
    let retry = custom(
        crate::session_engine::messages::PROVIDER_RETRY_OUTCOME_CUSTOM_TYPE,
        true,
        &json!({ "success": true, "attempts": 2, "finalError": "429" }),
    );
    assert!(crate::session_engine::messages::loop_convert_to_llm(vec![retry.clone()]).is_empty());
    let messages = vec![
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
        retry.clone(),
        assistant(&["c3"]),
        end("c3"),
    ];
    assert_eq!(
        fold_view(messages, BUDGET),
        vec![
            assistant(&["c1"]),
            begin("c1"),
            retry,
            assistant(&["c3"]),
            end("c3"),
        ]
    );
}

#[test]
fn an_unmatched_end_or_an_open_begin_leaves_out_nothing() {
    let unmatched = vec![
        assistant(&["c1"]),
        result("c1"),
        assistant(&["c2"]),
        end("c2"),
    ];
    assert_eq!(fold_view(unmatched.clone(), BUDGET), unmatched);
    let open = vec![
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
    ];
    assert_eq!(
        fold_view(open, BUDGET),
        vec![
            assistant(&["c1"]),
            begin("c1"),
            assistant(&["c2"]),
            with_line(result("c2"), REMINDER),
        ]
    );
}

#[test]
fn an_end_in_the_begin_group_leaves_out_nothing() {
    let messages = vec![
        assistant(&["c1", "c2"]),
        begin("c1"),
        end("c2"),
        assistant(&["c3"]),
        result("c3"),
    ];
    assert_eq!(fold_view(messages.clone(), BUDGET), messages);
}

#[test]
fn two_sequential_folds_both_close() {
    let messages = vec![
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
        assistant(&["c3"]),
        end("c3"),
        assistant(&["c4"]),
        begin("c4"),
        assistant(&["c5"]),
        result("c5"),
        assistant(&["c6"]),
        end("c6"),
        assistant(&["c7"]),
        result("c7"),
    ];
    assert_eq!(
        fold_view(messages, BUDGET),
        vec![
            assistant(&["c1"]),
            begin("c1"),
            assistant(&["c3"]),
            end("c3"),
            assistant(&["c4"]),
            begin("c4"),
            assistant(&["c6"]),
            end("c6"),
            assistant(&["c7"]),
            result("c7"),
        ]
    );
}

#[test]
fn the_reminder_becomes_the_over_budget_line_past_max_fold_tokens() {
    let messages = vec![
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
        assistant(&["c3"]),
        result("c3"),
    ];
    // The work after the begin group: two assistant messages and two results.
    let first_result_tokens: u64 = messages[2..4]
        .iter()
        .map(|message| pa_types::usage::estimate_tokens(&serde_json::to_value(message).unwrap()))
        .sum();
    let total: u64 = messages[2..]
        .iter()
        .map(|message| pa_types::usage::estimate_tokens(&serde_json::to_value(message).unwrap()))
        .sum();
    // A budget between the two running totals: the first result is inside it,
    // the second is past it.
    assert_eq!(
        fold_view(messages, first_result_tokens),
        vec![
            assistant(&["c1"]),
            begin("c1"),
            assistant(&["c2"]),
            with_line(result("c2"), REMINDER),
            assistant(&["c3"]),
            with_line(
                result("c3"),
                &format!(
                    "[fold over budget: explore, ~{total} tokens] Stop and call await fold.end(result) now, stating the progress made and the work still pending."
                ),
            ),
        ]
    );
}

#[test]
fn a_result_keeps_its_bytes_as_the_conversation_grows() {
    let mut messages = vec![
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
    ];
    let first = serde_json::to_vec(&fold_view(messages.clone(), BUDGET)).unwrap();
    messages.extend([assistant(&["c3"]), result("c3")]);
    let grown = fold_view(messages.clone(), BUDGET);
    assert_eq!(serde_json::to_vec(&grown[..4]).unwrap(), first);
    // A later void keeps the lines already sent.
    messages.push(user("stop"));
    let voided = fold_view(messages, BUDGET);
    assert_eq!(
        serde_json::to_vec(&voided[..6]).unwrap(),
        serde_json::to_vec(&grown).unwrap()
    );
}

#[test]
fn in_fold_results_carry_the_reminder_but_the_begin_and_end_cells_do_not() {
    // The begin cell shares its message with a sibling call: the whole
    // begin group is outside the fold, so neither result gets a line.
    let open = vec![
        user("go"),
        assistant(&["c1", "b1"]),
        begin("c1"),
        result("b1"),
        assistant(&["c2"]),
        result("c2"),
    ];
    assert_eq!(
        fold_view(open.clone(), BUDGET),
        vec![
            user("go"),
            assistant(&["c1", "b1"]),
            begin("c1"),
            result("b1"),
            assistant(&["c2"]),
            with_line(result("c2"), REMINDER),
        ]
    );
    let mut closed = open;
    closed.extend([assistant(&["c3"]), end("c3")]);
    assert_eq!(
        fold_view(closed, BUDGET),
        vec![
            user("go"),
            assistant(&["c1", "b1"]),
            begin("c1"),
            result("b1"),
            assistant(&["c3"]),
            end("c3"),
        ]
    );
}
