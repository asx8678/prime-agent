//! `fold.begin` / `fold.end`: the host side of the `fold` skill, registered
//! only while `contextFolding.enabled` is on. The host decides from the live
//! conversation (no state of its own, so a resume or a cell that dies after
//! the answer cannot disagree with the log) and answers the text the skill
//! prints, plus the marker the skill emits on its cell when one applies.
//! Marker format and texts live here, in one place.

use std::sync::{Arc, Weak};

use serde_json::{json, Value};

use crate::kernel::shared::{host_handler, HostRequestHandlers};
use crate::session_engine::turn_boundary::TurnBoundaryRequests;

use super::{Close, Folds};

const DESCRIPTION_CHARS: usize = 80;
const GOAL_CHARS: usize = 2_000;
const RESULT_CHARS: usize = 16_384;

/// Register `fold.begin` and `fold.end` over the session the turn-boundary
/// requests are bound to.
pub(crate) fn register_handlers(
    handlers: &mut HostRequestHandlers,
    requests: &Arc<TurnBoundaryRequests>,
    max_fold_tokens: u64,
) {
    let weak = Arc::downgrade(requests);
    handlers.register(
        "fold.begin",
        host_handler(move |payload| {
            let requests = weak.clone();
            Box::pin(async move {
                let description = text_field(&payload.data, "fold.begin", "description", DESCRIPTION_CHARS)?;
                if description.contains('\n') {
                    anyhow::bail!("fold.begin description must be one line");
                }
                let goal = text_field(&payload.data, "fold.begin", "goal", GOAL_CHARS)?;
                let messages = live_messages(&requests).await?;
                Ok(match Folds::scan(&messages, max_fold_tokens).open {
                    Some(open) => json!({
                        "text": format!(
                            "Fold not opened: a fold is already open: {}; finish it with fold.end before starting another.",
                            open.description
                        ),
                    }),
                    None => json!({
                        "text": format!(
                            "Fold opened: {description}. Work only on this goal. Finish with await fold.end(result), where result states the outcome, files changed or created, environment changes, key findings, assumptions and open questions. Everything until fold.end will be hidden from later turns; only the result stays. Keep anything you still need in kernel variables or files."
                        ),
                        "marker": { "kind": "begin", "description": description, "goal": goal },
                    }),
                })
            })
        }),
    );
    let weak = Arc::downgrade(requests);
    handlers.register(
        "fold.end",
        host_handler(move |payload| {
            let requests = weak.clone();
            Box::pin(async move {
                let result = text_field(&payload.data, "fold.end", "result", RESULT_CHARS)?;
                let messages = live_messages(&requests).await?;
                Ok(match Close::of(&messages, max_fold_tokens) {
                    Close::Closes {
                        description,
                        messages,
                        tokens,
                        over_budget,
                    } => json!({
                        "text": if messages == 0 {
                            format!(
                                "Fold closed: {description} (no work between the begin and end cells, so nothing was hidden).\nResult:\n{result}"
                            )
                        } else {
                            format!(
                                "Fold closed: {description} ({messages} messages, ~{tokens} tokens hidden from later turns).\nResult:\n{result}"
                            )
                        },
                        "marker": {
                            "kind": "end", "description": description, "result": result,
                            "folded": true, "messages": messages, "tokens": tokens,
                            "overBudget": over_budget,
                        },
                    }),
                    Close::Voided { description } => json!({
                        "text": "Fold not closed: new input arrived during the fold, so nothing was hidden.",
                        "marker": {
                            "kind": "end", "description": description, "result": result,
                            "folded": false, "messages": 0, "tokens": 0, "overBudget": false,
                        },
                    }),
                    Close::Refused(reason) => json!({ "text": format!("Fold not closed: {reason}.") }),
                })
            })
        }),
    );
}

/// A required, non-blank string argument of at most `max_chars` characters.
fn text_field(
    data: &Value,
    request_type: &str,
    key: &str,
    max_chars: usize,
) -> anyhow::Result<String> {
    let Some(text) = data.get(key).and_then(Value::as_str) else {
        anyhow::bail!("{request_type} {key} must be a string");
    };
    let text = text.trim();
    if text.is_empty() {
        anyhow::bail!("{request_type} {key} must not be empty");
    }
    if text.chars().count() > max_chars {
        anyhow::bail!("{request_type} {key} must be at most {max_chars} characters");
    }
    Ok(text.to_string())
}

/// The live conversation of the bound session, the running turn included.
async fn live_messages(
    requests: &Weak<TurnBoundaryRequests>,
) -> anyhow::Result<Vec<pa_agent::types::AgentMessage>> {
    let Some(runtime) = requests.upgrade().and_then(|requests| requests.bound()) else {
        anyhow::bail!("the session is not ready to fold");
    };
    Ok(runtime.agent.state().await.messages)
}
