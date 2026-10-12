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

/// Ask the `fold.*` handler `request_type` with `data`, over a session
/// whose live conversation is `messages`.
async fn ask(
    request_type: &str,
    data: Value,
    messages: Vec<AgentMessage>,
) -> anyhow::Result<Value> {
    use crate::session_engine::turn_boundary::{
        ModelInfo, TurnBoundaryRequests, TurnBoundaryRuntime,
    };
    use pa_agent::agent::{Agent, AgentInitialState, AgentOptions};

    let requests = std::sync::Arc::new(TurnBoundaryRequests::new());
    let mut handlers = crate::kernel::shared::HostRequestHandlers::new();
    super::register_handlers(&mut handlers, &requests, BUDGET);
    let model = pa_agent::types::Model {
        id: "m".into(),
        name: "m".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 100_000,
        max_tokens: 100,
        max_tokens_explicit: false,
    };
    requests.bind(TurnBoundaryRuntime {
        agent: std::sync::Arc::new(Agent::new(AgentOptions {
            initial_state: AgentInitialState {
                model: Some(model),
                messages: Some(messages),
                ..Default::default()
            },
            ..Default::default()
        })),
        session: std::sync::Arc::new(tokio::sync::Mutex::new(
            crate::session::manager::SessionManager::in_memory(std::path::Path::new("/tmp")),
        )),
        context_window: Some(100_000),
        model_info: ModelInfo {
            id: "m".to_string(),
            provider: "test".to_string(),
            input: Vec::new(),
        },
    });
    let handler = handlers.get(request_type).expect("registered").clone();
    handler(crate::kernel::shared::HostRequestPayload {
        data,
        cell_source_code: None,
    })
    .await
}

fn begin_args() -> Value {
    json!({ "description": "explore", "goal": "find the bug" })
}

#[tokio::test]
async fn begin_opens_a_fold_and_is_refused_while_one_is_open() {
    let answer = ask(
        "fold.begin",
        begin_args(),
        vec![user("go"), assistant(&["c1"])],
    )
    .await
    .unwrap();
    assert_eq!(
        answer["marker"],
        json!({ "kind": "begin", "description": "explore", "goal": "find the bug" })
    );
    assert!(answer["text"]
        .as_str()
        .unwrap()
        .starts_with("Fold opened: explore. Work only on this goal."));

    let open = vec![
        user("go"),
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
    ];
    let answer = ask(
        "fold.begin",
        json!({ "description": "next", "goal": "g" }),
        open,
    )
    .await
    .unwrap();
    assert_eq!(
        answer,
        json!({ "text": "Fold not opened: a fold is already open: explore; finish it with fold.end before starting another." })
    );
}

#[tokio::test]
async fn end_without_an_open_begin_answers_not_folded() {
    // The begin was summarized away by compaction, or never made.
    let answer = ask(
        "fold.end",
        json!({ "result": "done" }),
        vec![user("go"), assistant(&["c1"])],
    )
    .await
    .unwrap();
    assert_eq!(
        answer,
        json!({ "text": "Fold not closed: no fold is open; compaction may have summarized it." })
    );
}

#[tokio::test]
async fn end_in_the_same_assistant_turn_as_its_begin_is_refused() {
    // The begin cell and the end cell are two calls of one assistant message.
    let answer = ask(
        "fold.end",
        json!({ "result": "done" }),
        vec![user("go"), assistant(&["c1", "c2"]), begin("c1")],
    )
    .await
    .unwrap();
    assert_eq!(
        answer,
        json!({ "text": "Fold not closed: nothing to fold yet: call fold.end in a later cell." })
    );
}

#[tokio::test]
async fn end_reports_the_hidden_span() {
    let work = vec![
        assistant(&["c2"]),
        result("c2"),
        assistant(&["c3"]),
        result("c3"),
    ];
    let tokens: u64 = work
        .iter()
        .map(|message| pa_types::usage::estimate_tokens(&serde_json::to_value(message).unwrap()))
        .sum();
    let mut messages = vec![user("go"), assistant(&["c1"]), begin("c1")];
    messages.extend(work);
    // The in-flight message that called fold.end; its result is not in yet.
    messages.push(assistant(&["c4"]));
    let answer = ask("fold.end", json!({ "result": "found it" }), messages)
        .await
        .unwrap();
    assert_eq!(
        answer,
        json!({
            "text": format!("Fold closed: explore (4 messages, ~{tokens} tokens hidden from later turns).\nResult:\nfound it"),
            "marker": {
                "kind": "end", "description": "explore", "result": "found it", "folded": true,
                "messages": 4, "tokens": tokens, "overBudget": false,
            },
        })
    );
}

#[tokio::test]
async fn end_reports_a_voided_fold() {
    let messages = vec![
        user("go"),
        assistant(&["c1"]),
        begin("c1"),
        assistant(&["c2"]),
        result("c2"),
        user("actually, do something else"),
        assistant(&["c3"]),
    ];
    let answer = ask("fold.end", json!({ "result": "partial" }), messages.clone())
        .await
        .unwrap();
    assert_eq!(answer["marker"]["folded"], json!(false));
    assert_eq!(
        answer["text"],
        json!("Fold not closed: new input arrived during the fold, so nothing was hidden.")
    );
    // The voided fold is not open: a new one can begin.
    let answer = ask("fold.begin", begin_args(), messages).await.unwrap();
    assert!(answer.get("marker").is_some(), "{answer}");
}

#[tokio::test]
async fn begin_and_end_arguments_are_required_and_bounded() {
    let refused = [
        (
            "fold.begin",
            json!({ "goal": "g" }),
            "description must be a string",
        ),
        (
            "fold.begin",
            json!({ "description": "x" }),
            "goal must be a string",
        ),
        (
            "fold.begin",
            json!({ "description": "  ", "goal": "g" }),
            "description must not be empty",
        ),
        (
            "fold.begin",
            json!({ "description": "two\nlines", "goal": "g" }),
            "description must be one line",
        ),
        (
            "fold.begin",
            json!({ "description": "x".repeat(81), "goal": "g" }),
            "description must be at most 80 characters",
        ),
        (
            "fold.begin",
            json!({ "description": "x", "goal": "g".repeat(2_001) }),
            "goal must be at most 2000 characters",
        ),
        ("fold.end", json!({}), "result must be a string"),
        (
            "fold.end",
            json!({ "result": "r".repeat(16_385) }),
            "result must be at most 16384 characters",
        ),
    ];
    for (request_type, data, error) in refused {
        let answer = ask(request_type, data, vec![user("go")]).await;
        assert_eq!(
            answer.map_err(|error| error.to_string()),
            Err(format!("{request_type} {error}"))
        );
    }
}
