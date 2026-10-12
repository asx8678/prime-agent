//! The compaction executor: assemble and run the summarization request
//! (via pa-ai's completion facade).

use super::compaction::{build_summarization_prompt, CutPointResult};
use super::compaction_utils::{
    compute_file_lists, extract_file_ops_from_message, format_file_operations, FileOperations,
};
use super::messages::convert_to_llm;
use pa_types::ai::{AssistantMessage, TextContent, UserContent, UserContentBlock, UserMessage};
use pa_types::session::{AgentMessage, CompactionEntry, FileEntry};
use std::fmt::Write as _;

/// Details stored on the compaction entry for file tracking.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// The result of running a compaction.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    pub usage: Option<pa_types::ai::Usage>,
}

/// How the summarizer is invoked (test seam over pa-ai completion).
pub type SummarizerFn = Box<
    dyn FnOnce(
            pa_types::ai::Model,
            Vec<AgentMessage>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<AssistantMessage>> + Send>,
        > + Send,
>;

/// Assemble the summarization messages for the conversation slice.
/// `recent_state_anchor` is the newest retained assistant text: it rides after
/// the previous summary, so the update summary cannot lag behind the kept tail.
#[must_use]
pub fn build_summarization_request(
    messages: &[AgentMessage],
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
    recent_state_anchor: Option<&str>,
    #[allow(unused_variables)] reserve_tokens: u64,
) -> Vec<AgentMessage> {
    let llm_messages = convert_to_llm(messages);
    let conversation_text = super::compaction_utils::serialize_conversation(&llm_messages);
    let mut prompt_text = format!("<conversation>\n{conversation_text}\n</conversation>\n\n");
    if let Some(previous_summary) = previous_summary {
        let _ = write!(
            prompt_text,
            "<previous-summary>\n{previous_summary}\n</previous-summary>\n\n"
        );
    }
    if let Some(recent_state_anchor) = recent_state_anchor {
        let _ = write!(
            prompt_text,
            "<recent-state-anchor>\nNewest assistant message that stays retained below the summary. The conversation to summarize is older than this anchor; the retained messages below are authoritative, so treat this anchor, not the conversation above, as the current state.\n\n{recent_state_anchor}\n</recent-state-anchor>\n\n"
        );
    }
    prompt_text.push_str(&build_summarization_prompt(
        custom_instructions,
        previous_summary,
    ));
    vec![AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![UserContentBlock::Text(TextContent {
            text: prompt_text,
            text_signature: None,
            rest: serde_json::Map::default(),
        })]),
        timestamp: 0,
        rest: serde_json::Map::default(),
    })]
}

/// One resolved summarizer wire call: the summary text and what it
/// billed; the no-history split arm has no wire call (`usage: None`).
#[derive(Debug, Clone, PartialEq)]
pub struct SummarySlice {
    pub summary: String,
    pub usage: Option<pa_types::ai::Usage>,
}

/// The literal history stand-in for a split turn whose kept cut leaves
/// no history to summarize (no wire call).
pub const NO_PRIOR_HISTORY: &str = "No prior history.";

/// The merged summary of a split turn: history summary, split marker,
/// then turn-prefix summary.
#[must_use]
pub fn split_summary(history: &str, turn_prefix: &str) -> String {
    format!("{history}\n\n---\n\n**Turn Context (split turn):**\n\n{turn_prefix}")
}

/// The serialized prefix conversation under the turn-prefix instruction —
/// no custom instructions, no previous summary, no kernel note.
#[must_use]
pub fn build_turn_prefix_request(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    let llm_messages = convert_to_llm(messages);
    let conversation_text = super::compaction_utils::serialize_conversation(&llm_messages);
    let prompt_text = format!(
        "<conversation>\n{conversation_text}\n</conversation>\n\n{}",
        super::compaction::TURN_PREFIX_SUMMARIZATION_PROMPT
    );
    vec![AgentMessage::User(UserMessage {
        content: UserContent::Blocks(vec![UserContentBlock::Text(TextContent {
            text: prompt_text,
            text_signature: None,
            rest: serde_json::Map::default(),
        })]),
        timestamp: 0,
        rest: serde_json::Map::default(),
    })]
}

/// The wire-fallback telemetry seam: called once per session-model retry
/// with whether the retry succeeded (primitives only, never content).
pub type ModelFallbackHook = std::sync::Arc<dyn Fn(bool) + Send + Sync>;

/// The session-model target a routed one-off call can fall back to.
#[derive(Clone)]
pub struct SessionModelFallback {
    pub model: pa_types::ai::Model,
    pub api_key: Option<String>,
    /// The session model's own merged request headers (the registry's team
    /// header, exactly like the session's requests).
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    /// The routed model's base headers, before a summary slice merged its
    /// request id over them: the retry carries every header the slice added
    /// on top of the session headers.
    pub routed_base_headers: Option<std::collections::BTreeMap<String, String>>,
    /// Adoption telemetry for the retry.
    pub on_fallback: Option<ModelFallbackHook>,
}

impl SessionModelFallback {
    /// The retry's headers: the session model's own, plus every entry the
    /// summary slice merged over the routed base (its request id).
    fn retry_headers(
        &self,
        sent: Option<&std::collections::BTreeMap<String, String>>,
    ) -> Option<std::collections::BTreeMap<String, String>> {
        let mut headers = self.headers.clone().unwrap_or_default();
        for (key, value) in sent.into_iter().flatten() {
            let base = self
                .routed_base_headers
                .as_ref()
                .and_then(|base| base.get(key));
            if base != Some(value) {
                headers.insert(key.clone(), value.clone());
            }
        }
        (!headers.is_empty()).then_some(headers)
    }
}

/// Whether the routed model is the session model (same provider and id):
/// a same-model wire failure would recur, so the fallback is skipped.
#[must_use]
pub fn is_session_model(routed: &pa_types::ai::Model, session: &pa_types::ai::Model) -> bool {
    routed.provider == session.provider && routed.id == session.id
}

/// [`complete_summary_call`] with a session-model fallback (codex
/// `compact_model_fallback.rs`): when the routed model differs from the
/// session model and the wire call fails, retry once on the session model
/// with the session key and headers (plus the slice's request id). An
/// aborted run never retries. The retry re-clones the request, so the
/// routed call owns its copy.
///
/// # Errors
///
/// Returns the routed call's error when there is no fallback (same model,
/// no session target, or an aborted run); otherwise the session retry's
/// error.
#[allow(clippy::too_many_arguments)] // complete_summary_call's 7 + the fallback target and the abort signal
pub async fn complete_summary_call_with_fallback(
    routed_model: &pa_types::ai::Model,
    routed_api_key: Option<String>,
    routed_headers: Option<std::collections::BTreeMap<String, String>>,
    max_tokens: u64,
    request_messages: Vec<AgentMessage>,
    on_delta: Option<SummaryDeltaSink>,
    failure: &'static str,
    fallback: Option<SessionModelFallback>,
    abort: Option<&pa_agent::abort::AbortSignal>,
) -> anyhow::Result<SummarySlice> {
    // The routed call owns the original request; the retry needs its own.
    let retry_messages = request_messages.clone();
    let retry_delta = on_delta.clone();
    let sent_headers = routed_headers.clone();
    match complete_summary_call(
        routed_model,
        routed_api_key,
        routed_headers,
        max_tokens,
        request_messages,
        on_delta,
        failure,
    )
    .await
    {
        Ok(slice) => Ok(slice),
        Err(error) => {
            let Some(fallback) = fallback else {
                return Err(error);
            };
            // A same-model failure would recur; an aborted run discards
            // whatever a retry produced, so it never spends one.
            if is_session_model(routed_model, &fallback.model)
                || abort.is_some_and(pa_agent::abort::AbortSignal::is_aborted)
            {
                return Err(error);
            }
            eprintln!(
                "Warning: auxiliary model \"{}/{}\" failed for the summary wire call; using the session model.",
                routed_model.provider,
                routed_model.id
            );
            let headers = fallback.retry_headers(sent_headers.as_ref());
            let retried = complete_summary_call(
                &fallback.model,
                fallback.api_key,
                headers,
                max_tokens,
                retry_messages,
                retry_delta,
                failure,
            )
            .await;
            if let Some(on_fallback) = &fallback.on_fallback {
                on_fallback(retried.is_ok());
            }
            retried
        }
    }
}

/// The live summary-delta sink: called with every text delta the summarizer
/// streams, in arrival order. Fire-and-forget (emissions never gate the run);
/// the final summary comes from the terminal assistant message.
pub type SummaryDeltaSink = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// Run one summarizer wire call through `pa_ai::complete_simple`: `headers`
/// are the routed model's merged request headers; `on_delta` is the live sink
/// ([`SummaryDeltaSink`]); `failure` labels the error-stop bail.
///
/// # Errors
///
/// Returns an error when the summarizer wire call fails, or when its reply
/// stops with an error (labeled with `failure`).
pub async fn complete_summary_call(
    model: &pa_types::ai::Model,
    api_key: Option<String>,
    headers: Option<std::collections::BTreeMap<String, String>>,
    max_tokens: u64,
    request_messages: Vec<AgentMessage>,
    on_delta: Option<SummaryDeltaSink>,
    failure: &'static str,
) -> anyhow::Result<SummarySlice> {
    let messages = request_messages
        .into_iter()
        .filter_map(|message| match message {
            AgentMessage::User(user) => Some(pa_types::ai::Message::User(user)),
            AgentMessage::Assistant(assistant) => Some(pa_types::ai::Message::Assistant(assistant)),
            _ => None,
        })
        .collect();
    let context = pa_types::ai::Context {
        system_prompt: Some(super::compaction_utils::SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        messages,
        tools: None,
    };
    let stream_options =
        pa_ai::types::SimpleStreamOptions::from_base(pa_ai::types::StreamOptions {
            max_tokens: Some(max_tokens),
            api_key,
            headers: headers.map(|headers| headers.into_iter().collect()),
            // One-off summaries never read the prompt cache back, so they opt
            // out entirely (pi `completeSummarization`: cacheRetention "none"):
            // no cache writes, and a fresh routing id keeps the session's cache
            // namespace clean.
            cache_retention: Some(pa_types::ai::CacheRetention::None),
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            ..Default::default()
        });
    let assistant = match on_delta {
        None => pa_ai::complete_simple(model, &context, Some(stream_options)).await?,
        Some(on_delta) => {
            // The live path rides the same provider stream: every text delta
            // forwards to the sink, and the terminal event's message is the
            // summary; thinking deltas stay off the sink.
            let mut stream = pa_ai::stream_simple(model, &context, Some(stream_options))?;
            while let Some(event) = stream.next_event().await {
                if let pa_types::ai::AssistantMessageEvent::TextDelta { delta, .. } = &event {
                    on_delta(delta);
                }
            }
            stream.result().await
        }
    };
    if assistant.stop_reason == pa_types::ai::StopReason::Error {
        anyhow::bail!(
            "{failure}: {}",
            assistant
                .error_message
                .as_deref()
                .unwrap_or("Unknown error")
        );
    }
    let summary = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            pa_types::ai::AssistantContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(SummarySlice {
        summary,
        usage: Some(assistant.usage),
    })
}

/// Sum one wire call's billed usage into a total. Token fields saturate
/// at `u64::MAX`: JS numbers saturate to `Infinity`, and one overflowing
/// record must never panic a whole-file reader.
pub fn add_assistant_usage(total: &mut pa_types::ai::Usage, usage: &pa_types::ai::Usage) {
    total.input = total.input.saturating_add(usage.input);
    total.output = total.output.saturating_add(usage.output);
    total.cache_read = total.cache_read.saturating_add(usage.cache_read);
    total.cache_write = total.cache_write.saturating_add(usage.cache_write);
    total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
    let add_cost = |left: pa_types::JsNumber, right: pa_types::JsNumber| {
        pa_types::JsNumber::from(left.as_f64() + right.as_f64())
    };
    total.cost.input = add_cost(total.cost.input, usage.cost.input);
    total.cost.output = add_cost(total.cost.output, usage.cost.output);
    total.cost.cache_read = add_cost(total.cost.cache_read, usage.cost.cache_read);
    total.cost.cache_write = add_cost(total.cost.cache_write, usage.cost.cache_write);
    total.cost.total = add_cost(total.cost.total, usage.cost.total);
}

/// Remove one usage block from a total, clamping every field at zero
/// (absorbs attribution drift).
pub fn subtract_assistant_usage(total: &mut pa_types::ai::Usage, usage: &pa_types::ai::Usage) {
    total.input = total.input.saturating_sub(usage.input);
    total.output = total.output.saturating_sub(usage.output);
    total.cache_read = total.cache_read.saturating_sub(usage.cache_read);
    total.cache_write = total.cache_write.saturating_sub(usage.cache_write);
    total.total_tokens = total.total_tokens.saturating_sub(usage.total_tokens);
    let sub_cost = |left: pa_types::JsNumber, right: pa_types::JsNumber| {
        pa_types::JsNumber::from((left.as_f64() - right.as_f64()).max(0.0))
    };
    total.cost.input = sub_cost(total.cost.input, usage.cost.input);
    total.cost.output = sub_cost(total.cost.output, usage.cost.output);
    total.cost.cache_read = sub_cost(total.cost.cache_read, usage.cost.cache_read);
    total.cost.cache_write = sub_cost(total.cost.cache_write, usage.cost.cache_write);
    total.cost.total = sub_cost(total.cost.total, usage.cost.total);
}

/// The compaction's billed usage summed over its wire calls; a run with
/// no wire calls records no usage.
pub fn summed_usage(slices: &[SummarySlice]) -> Option<pa_types::ai::Usage> {
    let mut total: Option<pa_types::ai::Usage> = None;
    for slice in slices {
        if let Some(usage) = &slice.usage {
            let total = total.get_or_insert_with(pa_types::ai::Usage::default);
            add_assistant_usage(total, usage);
        }
    }
    total
}

/// File operations preserved across prior compactions plus current messages.
fn extract_file_operations(
    messages: &[AgentMessage],
    entries: &[FileEntry],
    prev_compaction_index: Option<usize>,
) -> FileOperations {
    let mut ops = FileOperations::default();
    if let Some(index) = prev_compaction_index {
        if let Some(FileEntry::Compaction { payload, .. }) = entries.get(index) {
            if payload.from_hook != Some(true) {
                if let Some(details) = payload.details.clone() {
                    if let Ok(details) = serde_json::from_value::<CompactionDetails>(details) {
                        ops.read.extend(details.read_files);
                        ops.edited.extend(details.modified_files);
                    }
                }
            }
        }
    }
    for message in messages {
        extract_file_ops_from_message(message, &mut ops);
    }
    ops
}

/// Inputs to a compaction run.
pub struct CompactRequest<'a> {
    pub messages: &'a [AgentMessage],
    pub cut: &'a CutPointResult,
    pub first_kept_entry_id: &'a str,
    pub tokens_before: u64,
    /// `/compact <instructions>` guidance.
    pub custom_instructions: Option<&'a str>,
    /// Previous summary for update-mode summarization.
    pub previous_summary: Option<&'a str>,
    /// Newest retained assistant text anchoring the summary to the
    /// kept-tail state.
    pub recent_state_anchor: Option<&'a str>,
    pub reserve_tokens: u64,
    pub model: pa_types::ai::Model,
}

/// Run compaction over a conversation slice: summarize the dropped prefix,
/// keeping from `first_kept_entry_id`. `summarize` performs the model call.
///
/// # Errors
///
/// Returns the `summarize` call's error when the summarization fails.
pub async fn compact_with(
    request: CompactRequest<'_>,
    summarize: SummarizerFn,
) -> anyhow::Result<CompactionResult> {
    let CompactRequest {
        messages,
        cut,
        first_kept_entry_id,
        tokens_before,
        custom_instructions,
        previous_summary,
        recent_state_anchor,
        reserve_tokens,
        model,
    } = request;
    let summarized = &messages[..cut.first_kept_entry_index.min(messages.len())];
    let request_messages = build_summarization_request(
        summarized,
        custom_instructions,
        previous_summary,
        recent_state_anchor,
        reserve_tokens,
    );
    let assistant = summarize(model, request_messages).await?;
    let usage = (assistant.usage.total_tokens > 0).then_some(assistant.usage);
    let summary = assistant
        .content
        .iter()
        .filter_map(|block| match block {
            pa_types::ai::AssistantContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CompactionResult {
        summary,
        first_kept_entry_id: first_kept_entry_id.to_string(),
        tokens_before,
        usage,
    })
}

/// The compaction entry to persist for a result. Every Rust compaction
/// records `fromHook: false` — the TS durable value, never a missing key.
pub fn compaction_entry_for(
    result: &CompactionResult,
    details: &CompactionDetails,
    custom_instructions: Option<&str>,
    harness_digest: Option<String>,
    harness_state_fingerprint: Option<String>,
) -> CompactionEntry {
    CompactionEntry {
        summary: result.summary.clone(),
        first_kept_entry_id: result.first_kept_entry_id.clone(),
        tokens_before: result.tokens_before,
        details: Some(serde_json::to_value(details).unwrap_or_default()),
        from_hook: Some(false),
        custom_instructions: custom_instructions.map(str::to_string),
        usage: result.usage,
        harness_digest,
        harness_state_fingerprint,
        structural_state: None,
    }
}

/// Full-file-list details for a compact run (prev compaction ops + messages).
#[must_use]
pub fn details_for(
    messages: &[AgentMessage],
    entries: &[FileEntry],
    prev_compaction_index: Option<usize>,
) -> CompactionDetails {
    let ops = extract_file_operations(messages, entries, prev_compaction_index);
    let (read_files, modified_files) = compute_file_lists(&ops);
    CompactionDetails {
        read_files,
        modified_files,
    }
}

/// The XML file-ops block appended to a summary presentation.
#[must_use]
pub fn file_ops_block(read_files: &[String], modified_files: &[String]) -> String {
    format_file_operations(read_files, modified_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_types::session::EntryBase;

    fn user(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: UserContent::Text(text.to_string()),
            timestamp: 0,
            rest: serde_json::Map::default(),
        })
    }

    fn summary_assistant(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![pa_types::ai::AssistantContentBlock::Text(TextContent {
                text: text.to_string(),
                text_signature: None,
                rest: serde_json::Map::default(),
            })],
            api: "openai-completions".to_string(),
            provider: "test".to_string(),
            model: "m".to_string(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: pa_types::ai::Usage::default(),
            stop_reason: pa_types::ai::StopReason::Stop,
            stop_reason_raw: None,
            error_message: None,
            timestamp: 0,
            rest: serde_json::Map::default(),
        }
    }

    #[test]
    fn add_assistant_usage_saturates_token_totals() {
        let mut total = pa_types::ai::Usage {
            input: u64::MAX,
            output: 1,
            ..Default::default()
        };
        add_assistant_usage(
            &mut total,
            &pa_types::ai::Usage {
                input: 10,
                ..Default::default()
            },
        );
        assert_eq!(total.input, u64::MAX);
        assert_eq!(total.output, 1);
    }

    #[test]
    fn compaction_entry_records_the_ts_wire_fields() {
        let usage = pa_types::ai::Usage {
            input: 20,
            output: 10,
            cache_read: 80,
            cache_write: 0,
            total_tokens: 110,
            cost: pa_types::ai::UsageCost::default(),
        };
        let result = CompactionResult {
            summary: "the overflow summary".to_string(),
            first_kept_entry_id: "e4".to_string(),
            tokens_before: 214,
            usage: Some(usage),
        };
        let details = CompactionDetails {
            read_files: vec!["a.rs".to_string()],
            modified_files: vec![],
        };
        assert_eq!(
            compaction_entry_for(&result, &details, Some("focus"), None, None),
            CompactionEntry {
                summary: "the overflow summary".to_string(),
                first_kept_entry_id: "e4".to_string(),
                tokens_before: 214,
                details: Some(serde_json::json!({
                    "readFiles": ["a.rs"],
                    "modifiedFiles": [],
                })),
                from_hook: Some(false),
                custom_instructions: Some("focus".to_string()),
                usage: Some(usage),
                harness_digest: None,
                harness_state_fingerprint: None,
                structural_state: None,
            }
        );
    }

    /// The history request carries the recency anchor after the previous
    /// summary; a missing anchor adds no block.
    #[test]
    fn summarization_request_carries_the_recent_state_anchor() {
        let request = build_summarization_request(
            std::slice::from_ref(&user("the conversation")),
            None,
            Some("the previous summary"),
            Some("the newest kept-tail text"),
            1_000,
        );
        let AgentMessage::User(prompt) = &request[0] else {
            panic!("the summarization request is a single user message");
        };
        let text = prompt.content.text();
        let previous_end = text.find("</previous-summary>").expect("previous summary");
        let anchor_start = text.find("<recent-state-anchor>").expect("anchor block");
        let anchor_end = text.find("</recent-state-anchor>").expect("anchor close");
        assert!(previous_end < anchor_start && anchor_start < anchor_end);
        assert!(text.contains(
            "<recent-state-anchor>\nNewest assistant message that stays retained below the summary. The conversation to summarize is older than this anchor; the retained messages below are authoritative, so treat this anchor, not the conversation above, as the current state.\n\nthe newest kept-tail text\n</recent-state-anchor>\n\n"
        ));
        let request = build_summarization_request(
            std::slice::from_ref(&user("the conversation")),
            None,
            Some("the previous summary"),
            None,
            1_000,
        );
        let AgentMessage::User(prompt) = &request[0] else {
            panic!("the summarization request is a single user message");
        };
        assert!(!prompt.content.text().contains("<recent-state-anchor>"));
    }

    /// The serialized `details` block byte-matches the TS literal order
    /// (`readFiles` first): struct field order is the wire byte order.
    #[test]
    fn details_serialize_in_the_ts_key_order() {
        let details = CompactionDetails {
            read_files: vec!["a.rs".to_string(), "b.rs".to_string()],
            modified_files: vec!["c.rs".to_string()],
        };
        assert_eq!(
            serde_json::to_string(&details).unwrap(),
            "{\"readFiles\":[\"a.rs\",\"b.rs\"],\"modifiedFiles\":[\"c.rs\"]}"
        );
    }

    #[tokio::test]
    async fn compact_summarizes_prefix_and_returns_result() {
        let messages = vec![user("one"), user("two"), user("three")];
        let entries: Vec<FileEntry> = messages
            .iter()
            .enumerate()
            .map(|(index, _)| FileEntry::Message {
                message: user(&format!("m{index}")),
                base: EntryBase {
                    id: Some(format!("e{index}")),
                    parent_id: None,
                    timestamp: Some("2024-01-01T00:00:00.000Z".to_string()),
                    rest: serde_json::Map::default(),
                },
            })
            .collect();
        let cut = CutPointResult {
            first_kept_entry_index: 1,
            turn_start_index: None,
            is_split_turn: false,
        };
        let model: pa_types::ai::Model = serde_json::from_value(serde_json::json!({
            "id": "m", "name": "m", "api": "openai-completions", "provider": "test",
            "baseUrl": "http://localhost", "reasoning": false, "input": ["text"],
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
            "contextWindow": 1000, "maxTokens": 100
        }))
        .unwrap();
        let summarize: SummarizerFn = Box::new(|_model, request| {
            Box::pin(async move {
                // The request only contains the dropped prefix.
                match &request[0] {
                    AgentMessage::User(user) => {
                        let text = user.content.text();
                        assert!(text.contains("<conversation>"));
                        assert!(text.contains("one"));
                        assert!(!text.contains("two"));
                    }
                    _ => panic!("expected user request"),
                }
                Ok(summary_assistant("## Goal\nship it"))
            })
        });
        let result = compact_with(
            CompactRequest {
                messages: &messages,
                cut: &cut,
                first_kept_entry_id: "e1",
                tokens_before: 1_000,
                custom_instructions: Some("focus"),
                previous_summary: None,
                recent_state_anchor: None,
                reserve_tokens: 1_000,
                model,
            },
            summarize,
        )
        .await
        .unwrap();
        assert!(result.summary.starts_with("## Goal"));
        assert_eq!(result.first_kept_entry_id, "e1");
        assert_eq!(result.tokens_before, 1_000);
        let details = details_for(&messages, &entries, None);
        let entry = compaction_entry_for(&result, &details, Some("focus"), None, None);
        assert_eq!(entry.summary, "## Goal\nship it");
        assert_eq!(entry.custom_instructions.as_deref(), Some("focus"));
    }

    /// The turn-prefix request never carries the checkpoint prompt, a
    /// previous summary, or the kernel note.
    #[test]
    fn turn_prefix_request_shape() {
        let messages = vec![user("big turn"), user("more of the turn")];
        let request = build_turn_prefix_request(&messages);
        let AgentMessage::User(user) = &request[0] else {
            panic!("expected user request");
        };
        let text = user.content.text();
        assert!(text.starts_with(
            "<conversation>\n[User]: big turn\n\n[User]: more of the turn\n</conversation>\n\n"
        ));
        assert!(text.contains("This is the PREFIX of a turn that was too large to keep."));
        assert!(text.ends_with("Be concise. Focus on what's needed to understand the kept suffix."));
        assert!(!text.contains("Create a structured context checkpoint summary"));
        assert!(!text.contains("<previous-summary>"));
        assert!(!text.contains("the Python kernel keeps running"));
    }

    #[test]
    fn split_summary_marker_format() {
        assert_eq!(
            split_summary("history summary", "turn prefix summary"),
            "history summary\n\n---\n\n**Turn Context (split turn):**\n\nturn prefix summary"
        );
        assert_eq!(NO_PRIOR_HISTORY, "No prior history.");
    }

    /// A call without usage contributes nothing; no calls means no
    /// recorded usage.
    #[test]
    fn usage_summing_over_slices() {
        let usage = |input: u64, output: u64, cost: f64| pa_types::ai::Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
            total_tokens: input + output,
            cost: pa_types::ai::UsageCost {
                input: pa_types::JsNumber::from(cost),
                output: 0.0.into(),
                cache_read: 0.0.into(),
                cache_write: 0.0.into(),
                total: pa_types::JsNumber::from(cost),
            },
        };
        // Binary-exact costs so the sum assertion compares exactly.
        let (first_cost, second_cost) = (0.25, 0.5);
        let slice = |summary: &str, usage: Option<pa_types::ai::Usage>| SummarySlice {
            summary: summary.to_string(),
            usage,
        };
        let summed = summed_usage(&[
            slice("a", Some(usage(10, 5, first_cost))),
            slice("b", Some(usage(3, 2, second_cost))),
        ])
        .expect("usage recorded");
        assert_eq!(summed.input, 13);
        assert_eq!(summed.output, 7);
        assert_eq!(summed.total_tokens, 20);
        assert_eq!(summed.cost.input.as_f64(), 0.75);
        assert_eq!(summed.cost.total.as_f64(), 0.75);
        let mixed = summed_usage(&[slice("a", None), slice("b", Some(usage(3, 2, 0.0)))])
            .expect("usage recorded");
        assert_eq!(mixed.total_tokens, 5);
        assert_eq!(summed_usage(&[slice("a", None)]), None);
    }

    #[test]
    fn summarization_request_shape() {
        let messages = vec![user("hello"), user("world")];
        let request = build_summarization_request(
            &messages,
            Some("be brief"),
            None,
            /*recent_state_anchor*/ None,
            1_000,
        );
        match &request[0] {
            AgentMessage::User(user) => {
                let text = user.content.text();
                assert!(text.starts_with(
                    "<conversation>\n[User]: hello\n\n[User]: world\n</conversation>"
                ));
                assert!(text.contains("<user-instructions>\nThe user provided these instructions"));
                assert!(text.ends_with("redefining them."));
            }
            _ => panic!("expected user message"),
        }
    }

    /// The one-off summarizer opts out of prompt caching (pi
    /// `completeSummarization` parity): the request carries
    /// `CacheRetention::None` and a fresh routing id, so it writes no cache
    /// entries and never pollutes the session's cache namespace.
    #[tokio::test]
    async fn one_off_summary_opts_out_of_prompt_caching() {
        type Captured = (Option<pa_types::ai::CacheRetention>, Option<String>);
        let registration =
            pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
                models: Some(vec![pa_ai::faux::FauxModelDefinition {
                    id: "cache-out-m".to_string(),
                    name: Some("Cache Out Model".to_string()),
                    reasoning: Some(false),
                    input: Some(vec![pa_types::ai::ModelInput::Text]),
                    cost: None,
                    context_window: Some(1_000),
                    max_tokens: Some(256),
                }]),
                ..Default::default()
            });
        let model = registration.get_model();
        let seen: std::sync::Arc<std::sync::Mutex<Vec<Captured>>> = std::sync::Arc::default();
        let recorder = seen.clone();
        registration.set_responses(vec![pa_ai::faux::FauxResponseStep::Factory(
            std::sync::Arc::new(
                move |_context: &pa_types::ai::Context,
                      options: Option<&pa_ai::types::StreamOptions>,
                      _call: u64,
                      _model: &pa_types::ai::Model| {
                    let captured = options.map_or((None, None), |options| {
                        (options.cache_retention, options.session_id.clone())
                    });
                    recorder.lock().unwrap().push(captured);
                    Ok(pa_ai::faux::faux_assistant_text_message(
                        "the summary",
                        pa_ai::faux::FauxAssistantMessageOptions::default(),
                    ))
                },
            ),
        )]);
        let slice = complete_summary_call(
            &model,
            None,
            None,
            64,
            vec![user("summarize this")],
            None,
            "test",
        )
        .await
        .unwrap();
        assert_eq!(slice.summary, "the summary");
        let seen = seen.lock().unwrap();
        let (retention, session_id) = seen[0].clone();
        assert_eq!(retention, Some(pa_types::ai::CacheRetention::None));
        assert!(session_id.is_some(), "one-off requests route on a fresh id");
        registration.unregister();
    }

    /// Serve one Anthropic Messages request on a local port: the request
    /// body lands in the returned channel, and the reply is a minimal text
    /// stream.
    async fn capturing_anthropic_server() -> (
        std::net::SocketAddr,
        tokio::sync::oneshot::Receiver<serde_json::Value>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (body_tx, body_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            // Read the head, then exactly Content-Length body bytes.
            let (head_end, content_length) = loop {
                let read = socket.read(&mut chunk).await.unwrap();
                assert!(read > 0, "the client closed before sending its request");
                request.extend_from_slice(&chunk[..read]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .map_or(0, |value| value.trim().parse::<usize>().unwrap());
                    break (end + 4, length);
                }
            };
            while request.len() < head_end + content_length {
                let read = socket.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..read]);
            }
            let body: serde_json::Value =
                serde_json::from_slice(&request[head_end..head_end + content_length]).unwrap();
            let _ = body_tx.send(body);
            let sse = [
                r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#,
                r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"the summary"}}"#,
                r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
                r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
                r#"event: message_stop
data: {"type":"message_stop"}"#,
            ]
            .join("\n\n")
                + "\n\n";
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{sse}",
                        sse.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        (addr, body_rx)
    }

    /// The PR-1 wire proof: the one-off summarizer's request body to an
    /// Anthropic Messages endpoint carries no `cache_control` anywhere (the
    /// system block and the last user message would otherwise be marked).
    #[tokio::test]
    async fn one_off_summary_sends_no_cache_control_on_the_wire() {
        let (addr, body) = capturing_anthropic_server().await;
        let model: pa_types::ai::Model = serde_json::from_value(serde_json::json!({
            "id": "claude-wire-test", "name": "Claude Wire Test",
            "api": "anthropic-messages", "provider": "anthropic",
            "baseUrl": format!("http://{addr}"), "reasoning": false, "input": ["text"],
            "cost": {"input": 3, "output": 15, "cacheRead": 0.3, "cacheWrite": 3.75},
            "contextWindow": 200_000, "maxTokens": 8_192
        }))
        .unwrap();
        let slice = complete_summary_call(
            &model,
            Some("sk-ant-api-test".to_string()),
            None,
            64,
            vec![user("summarize this")],
            None,
            "test",
        )
        .await
        .unwrap();
        assert_eq!(slice.summary, "the summary");
        let body = body.await.unwrap();
        assert!(
            body["system"].is_array() || body["system"].is_string(),
            "the summarizer system prompt is on the wire: {body}"
        );
        assert!(
            !body.to_string().contains("cache_control"),
            "no cache_control marker on the wire: {body}"
        );
    }

    /// A faux provider serving a session model and an auxiliary model.
    fn session_and_aux_models() -> (
        pa_ai::faux::FauxProviderRegistration,
        pa_types::ai::Model,
        pa_types::ai::Model,
    ) {
        let definition = |id: &str| pa_ai::faux::FauxModelDefinition {
            id: id.to_string(),
            name: Some(id.to_string()),
            reasoning: Some(false),
            input: Some(vec![pa_types::ai::ModelInput::Text]),
            cost: None,
            context_window: Some(4_000),
            max_tokens: Some(256),
        };
        let registration =
            pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
                models: Some(vec![definition("session-m"), definition("aux-m")]),
                ..Default::default()
            });
        let aux = registration
            .get_model_by_id("aux-m")
            .expect("aux registered");
        let session = registration
            .get_model_by_id("session-m")
            .expect("session model registered");
        (registration, aux, session)
    }

    /// One observed summary call: the model id and the request headers.
    type SeenCall = (String, std::collections::BTreeMap<String, String>);

    /// A scripted step that records the call, then fails on the wire
    /// (`failed`) or answers `text`.
    fn recording_step(
        seen: &std::sync::Arc<std::sync::Mutex<Vec<SeenCall>>>,
        failed: bool,
        text: &'static str,
    ) -> pa_ai::faux::FauxResponseStep {
        let seen = seen.clone();
        pa_ai::faux::FauxResponseStep::Factory(std::sync::Arc::new(
            move |_context: &pa_types::ai::Context,
                  options: Option<&pa_ai::types::StreamOptions>,
                  _call: u64,
                  model: &pa_types::ai::Model| {
                let headers = options
                    .and_then(|options| options.headers.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                seen.lock().unwrap().push((model.id.clone(), headers));
                let options = if failed {
                    pa_ai::faux::FauxAssistantMessageOptions {
                        stop_reason: Some(pa_types::ai::StopReason::Error),
                        error_message: Some("500: overloaded".to_string()),
                        ..Default::default()
                    }
                } else {
                    pa_ai::faux::FauxAssistantMessageOptions::default()
                };
                Ok(pa_ai::faux::faux_assistant_text_message(text, options))
            },
        ))
    }

    fn headers(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    /// The wire fallback: a routed model that fails on the wire is retried
    /// once on the session model, which succeeds (codex
    /// `compact_model_fallback.rs`). The retry carries the session model's
    /// own headers plus the slice's request id, never the routed model's
    /// headers, and reports the retry to the telemetry seam.
    #[tokio::test]
    async fn summary_wire_failure_falls_back_to_the_session_model() {
        let (registration, aux_model, session_model) = session_and_aux_models();
        let seen: std::sync::Arc<std::sync::Mutex<Vec<SeenCall>>> = std::sync::Arc::default();
        registration.set_responses(vec![
            recording_step(&seen, true, ""),
            recording_step(&seen, false, "the fallback summary"),
        ]);
        let reported: std::sync::Arc<std::sync::Mutex<Vec<bool>>> = std::sync::Arc::default();
        let report = reported.clone();
        let slice = complete_summary_call_with_fallback(
            &aux_model,
            None,
            // The routed base plus the slice's request id.
            Some(headers(&[
                ("x-team", "aux-team"),
                ("x-request-id", "slice-1"),
            ])),
            64,
            vec![user("summarize this")],
            None,
            "test",
            Some(SessionModelFallback {
                model: session_model,
                api_key: None,
                headers: Some(headers(&[("x-team", "session-team")])),
                routed_base_headers: Some(headers(&[("x-team", "aux-team")])),
                on_fallback: Some(std::sync::Arc::new(move |succeeded| {
                    report.lock().unwrap().push(succeeded);
                })),
            }),
            None,
        )
        .await
        .unwrap();
        assert_eq!(slice.summary, "the fallback summary");
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [
                (
                    "aux-m".to_string(),
                    headers(&[("x-team", "aux-team"), ("x-request-id", "slice-1")])
                ),
                (
                    "session-m".to_string(),
                    headers(&[("x-team", "session-team"), ("x-request-id", "slice-1")])
                ),
            ]
        );
        assert_eq!(reported.lock().unwrap().as_slice(), [true]);
        registration.unregister();
    }

    /// An aborted run never spends a retry: the routed failure surfaces
    /// after exactly one call, and the telemetry seam stays silent.
    #[tokio::test]
    async fn summary_wire_failure_on_an_aborted_run_does_not_retry() {
        let (registration, aux_model, session_model) = session_and_aux_models();
        let seen: std::sync::Arc<std::sync::Mutex<Vec<SeenCall>>> = std::sync::Arc::default();
        registration.set_responses(vec![
            recording_step(&seen, true, ""),
            recording_step(&seen, false, "never served"),
        ]);
        let reported: std::sync::Arc<std::sync::Mutex<Vec<bool>>> = std::sync::Arc::default();
        let report = reported.clone();
        let controller = pa_agent::abort::AbortController::new();
        controller.abort();
        let signal = controller.signal();
        let result = complete_summary_call_with_fallback(
            &aux_model,
            None,
            None,
            64,
            vec![user("summarize this")],
            None,
            "test",
            Some(SessionModelFallback {
                model: session_model,
                api_key: None,
                headers: None,
                routed_base_headers: None,
                on_fallback: Some(std::sync::Arc::new(move |succeeded| {
                    report.lock().unwrap().push(succeeded);
                })),
            }),
            Some(&signal),
        )
        .await;
        assert!(result.is_err(), "the routed failure surfaces");
        assert_eq!(seen.lock().unwrap().len(), 1, "exactly one call was made");
        assert!(reported.lock().unwrap().is_empty());
        registration.unregister();
    }

    /// A same-model failure does not retry: the fallback is skipped when the
    /// routed model IS the session model (the retry would fail the same way).
    #[tokio::test]
    async fn summary_wire_failure_same_model_does_not_retry() {
        let registration =
            pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
                models: Some(vec![pa_ai::faux::FauxModelDefinition {
                    id: "same-m".to_string(),
                    name: Some("Same Model".to_string()),
                    reasoning: Some(false),
                    input: Some(vec![pa_types::ai::ModelInput::Text]),
                    cost: None,
                    context_window: Some(4_000),
                    max_tokens: Some(256),
                }]),
                ..Default::default()
            });
        let model = registration.get_model();
        let call_count = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let counter = call_count.clone();
        registration.set_responses(vec![pa_ai::faux::FauxResponseStep::Factory(
            std::sync::Arc::new(
                move |_context: &pa_types::ai::Context,
                      _options: Option<&pa_ai::types::StreamOptions>,
                      _call: u64,
                      _model: &pa_types::ai::Model| {
                    *counter.lock().unwrap() += 1;
                    Ok(pa_ai::faux::faux_assistant_text_message(
                        "",
                        pa_ai::faux::FauxAssistantMessageOptions {
                            stop_reason: Some(pa_types::ai::StopReason::Error),
                            error_message: Some("500: overloaded".to_string()),
                            ..Default::default()
                        },
                    ))
                },
            ),
        )]);
        let result = complete_summary_call_with_fallback(
            &model,
            None,
            None,
            64,
            vec![user("summarize this")],
            None,
            "test",
            Some(SessionModelFallback {
                model: model.clone(),
                api_key: None,
                headers: None,
                routed_base_headers: None,
                on_fallback: None,
            }),
            None,
        )
        .await;
        assert!(result.is_err(), "the same-model failure is not retried");
        assert_eq!(*call_count.lock().unwrap(), 1, "exactly one call was made");
        registration.unregister();
    }
}
