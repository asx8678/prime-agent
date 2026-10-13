//! Branch summarization for tree navigation.

use std::collections::HashSet;

use pa_types::session::{AgentMessage, CompactionSummaryMessage, FileEntry};

use super::compaction::estimate_tokens;
use super::compaction_utils::{
    compute_file_lists, extract_file_ops_from_message, format_file_operations,
    serialize_conversation, FileOperations,
};
use super::messages::{convert_to_llm, BRANCH_SUMMARY_PREFIX, BRANCH_SUMMARY_SUFFIX};

/// Details stored on a branch summary entry.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// The result of generating a branch summary.
#[derive(Debug, Default, Clone)]
pub struct BranchSummaryResult {
    pub summary: Option<String>,
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
    pub aborted: bool,
    pub error: Option<String>,
    pub usage: Option<pa_types::ai::Usage>,
    /// The model that served the summary call: the caller persists it, so the
    /// cost fold bills the model that billed it, not the branch timeline.
    pub model: Option<(String, String)>,
}

/// Prepared summarization inputs.
#[derive(Debug, Default)]
pub struct BranchPreparation {
    pub messages: Vec<AgentMessage>,
    pub file_ops: FileOperations,
    pub total_tokens: u64,
}

/// Entry-path info between two tree positions.
#[derive(Debug, Default)]
pub struct CollectEntriesResult {
    pub entries: Vec<FileEntry>,
    pub common_ancestor_id: Option<String>,
}

fn parent_path(entries: &[FileEntry], leaf_id: &str) -> Vec<String> {
    let mut path = Vec::new();
    // A `parentId` cycle in a hand-edited or corrupt session file must end the walk.
    let mut visited = HashSet::new();
    let mut current = Some(leaf_id.to_string());
    while let Some(id) = current {
        if !visited.insert(id.clone()) {
            break;
        }
        let Some(entry) = entries.iter().find(|entry| entry.id() == Some(id.as_str())) else {
            break;
        };
        path.push(id.clone());
        current = entry.parent_id().map(str::to_string);
    }
    path
}

/// Entries to summarize when navigating old-leaf -> target: the old path back
/// to (excluding) the common ancestor with the target path.
pub fn collect_entries_for_branch_summary(
    entries: &[FileEntry],
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> CollectEntriesResult {
    let Some(old_leaf_id) = old_leaf_id else {
        return CollectEntriesResult::default();
    };
    let old_path: HashSet<String> = parent_path(entries, old_leaf_id).into_iter().collect();
    let target_path = parent_path(entries, target_id);
    let common_ancestor_id = target_path
        .iter()
        .rev()
        .find(|id| old_path.contains(*id))
        .cloned();
    let mut collected: Vec<FileEntry> = Vec::new();
    let mut visited = HashSet::new();
    let mut current = Some(old_leaf_id.to_string());
    while let Some(id) = current {
        if Some(&id) == common_ancestor_id.as_ref() || !visited.insert(id.clone()) {
            break;
        }
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.id() == Some(id.as_str()))
            .cloned()
        else {
            break;
        };
        current = entry.parent_id().map(str::to_string);
        collected.push(entry);
    }
    collected.reverse();
    CollectEntriesResult {
        entries: collected,
        common_ancestor_id,
    }
}

/// The message an entry contributes to summarizer input (compaction entries
/// become their summary message; bookkeeping entries contribute nothing).
fn get_message_from_entry(entry: &FileEntry) -> Option<AgentMessage> {
    match entry {
        FileEntry::Message { message, .. } => match message {
            // Tool results stay attached to their assistant tool call.
            AgentMessage::ToolResult(_) => None,
            _ => Some(message.clone()),
        },
        FileEntry::CustomMessage { payload, .. } => {
            if payload.custom_type == "harness_digest" {
                return None;
            }
            Some(AgentMessage::Custom(pa_types::session::CustomMessage {
                custom_type: payload.custom_type.clone(),
                content: payload.content.clone(),
                display: payload.display,
                details: payload.details.clone(),
                timestamp: super::super::session::timestamp_to_millis(entry.timestamp()),
                rest: serde_json::Map::default(),
            }))
        }
        FileEntry::BranchSummary { payload, .. } => Some(AgentMessage::BranchSummary(
            pa_types::session::BranchSummaryMessage {
                summary: payload.summary.clone(),
                from_id: payload.from_id.clone(),
                timestamp: super::super::session::timestamp_to_millis(entry.timestamp()),
            },
        )),
        FileEntry::Compaction { payload, .. } => {
            Some(AgentMessage::CompactionSummary(CompactionSummaryMessage {
                summary: payload.summary.clone(),
                tokens_before: payload.tokens_before,
                retained_message_count: None,
                custom_instructions: payload.custom_instructions.clone(),
                harness_digest: payload.harness_digest.clone(),
                harness_state_fingerprint: payload.harness_state_fingerprint.clone(),
                timestamp: super::super::session::timestamp_to_millis(entry.timestamp()),
            }))
        }
        _ => None,
    }
}

/// Prepare entries under a token budget: newest-to-oldest until the budget
/// is hit. File ops are collected from ALL entries (cumulative tracking).
#[must_use]
pub fn prepare_branch_entries(entries: &[FileEntry], token_budget: u64) -> BranchPreparation {
    let mut file_ops = FileOperations::default();
    // Cumulative tracking from prior branch summaries (never hooked ones).
    for entry in entries {
        if let FileEntry::BranchSummary { payload, .. } = entry {
            if payload.from_hook != Some(true) {
                if let Some(details) = payload.details.clone() {
                    if let Ok(details) = serde_json::from_value::<BranchSummaryDetails>(details) {
                        file_ops.read.extend(details.read_files);
                        file_ops.edited.extend(details.modified_files);
                    }
                }
            }
        }
    }
    let mut messages: Vec<AgentMessage> = Vec::new();
    let mut total_tokens = 0u64;
    for entry in entries.iter().rev() {
        let Some(message) = get_message_from_entry(entry) else {
            continue;
        };
        extract_file_ops_from_message(&message, &mut file_ops);
        let tokens = estimate_tokens(&message);
        if token_budget > 0 && total_tokens + tokens > token_budget {
            // Compaction/branch summaries squeeze in under 90% budget.
            let boundary = matches!(
                entry,
                FileEntry::Compaction { .. } | FileEntry::BranchSummary { .. }
            );
            if boundary && total_tokens < token_budget * 9 / 10 {
                messages.insert(0, message);
                total_tokens += tokens;
            }
            break;
        }
        messages.insert(0, message);
        total_tokens += tokens;
    }
    BranchPreparation {
        messages,
        file_ops,
        total_tokens,
    }
}

const BRANCH_SUMMARY_PREAMBLE: &str = "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n";

const BRANCH_SUMMARY_PROMPT: &str = "Create a structured summary of this conversation branch for context when returning later.\n\nUse this EXACT format:\n\n## Goal\n[What was the user trying to accomplish in this branch?]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Work that was started but not finished]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [What should happen next to continue this work]\n\nKeep each section concise. Preserve exact file paths, function names, and error messages.";

/// Build the branch-summary request messages for the model.
#[must_use]
pub fn build_branch_summary_request(
    entries: &[FileEntry],
    token_budget: u64,
    custom_instructions: Option<&str>,
    replace_instructions: bool,
) -> (Vec<AgentMessage>, BranchPreparation) {
    let preparation = prepare_branch_entries(entries, token_budget);
    let messages = if preparation.messages.is_empty() {
        Vec::new()
    } else {
        let llm_messages = convert_to_llm(&preparation.messages);
        let conversation_text = serialize_conversation(&llm_messages);
        let instructions = match (replace_instructions, custom_instructions) {
            (true, Some(custom)) => custom.to_string(),
            (false, Some(custom)) => {
                format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {custom}")
            }
            _ => BRANCH_SUMMARY_PROMPT.to_string(),
        };
        let prompt_text =
            format!("<conversation>\n{conversation_text}\n</conversation>\n\n{instructions}");
        vec![AgentMessage::User(pa_types::ai::UserMessage {
            content: pa_types::ai::UserContent::Text(prompt_text),
            timestamp: 0,
            rest: serde_json::Map::default(),
        })]
    };
    (messages, preparation)
}

/// Assemble the final summary text from a model response.
#[must_use]
pub fn finalize_branch_summary(
    response_text: &str,
    preparation: &BranchPreparation,
) -> BranchSummaryResult {
    let (read_files, modified_files) = compute_file_lists(&preparation.file_ops);
    let mut summary = format!("{BRANCH_SUMMARY_PREAMBLE}{response_text}");
    summary.push_str(&format_file_operations(&read_files, &modified_files));
    BranchSummaryResult {
        summary: Some(if summary.is_empty() {
            "No summary generated".to_string()
        } else {
            summary
        }),
        read_files,
        modified_files,
        aborted: false,
        error: None,
        usage: None,
        model: None,
    }
}

/// Options for one branch-summary generation run.
pub struct GenerateBranchSummaryOptions<'a> {
    pub model: &'a pa_types::ai::Model,
    pub api_key: Option<String>,
    pub custom_instructions: Option<&'a str>,
    /// Replace the default prompt instead of appending the custom focus.
    pub replace_instructions: bool,
    /// Tokens reserved for prompt + response (TS default 16384).
    pub reserve_tokens: u64,
    /// When present, the summary call resolves its model through the
    /// `auxiliaryModel` setting with a context-window fit check.
    pub auxiliary: Option<&'a super::auxiliary_model::AuxiliaryModelContext>,
    /// Adoption telemetry for the routed-model wire fallback: called once
    /// per session-model retry with whether it succeeded.
    pub on_model_fallback: Option<super::compaction_exec::ModelFallbackHook>,
}

/// The default reserve budget.
pub const DEFAULT_BRANCH_RESERVE_TOKENS: u64 = 16_384;

/// The summarizer call cap.
const BRANCH_SUMMARY_MAX_TOKENS: u64 = 2048;

/// Estimate the context window the branch-summary request needs, using the
/// same entry-slicing budget and prompt builder as the wire call. The larger
/// of the request body and the reserve decides whether the auxiliary model
/// fits; a non-positive budget is treated as unlimited.
#[must_use]
pub fn estimate_branch_summary_request_tokens(
    entries: &[FileEntry],
    context_window: u64,
    reserve_tokens: u64,
    custom_instructions: Option<&str>,
    replace_instructions: bool,
) -> u64 {
    let window = if context_window > 0 {
        context_window
    } else {
        128_000
    };
    let token_budget = window.saturating_sub(reserve_tokens);
    let (request, _) = build_branch_summary_request(
        entries,
        token_budget,
        custom_instructions,
        replace_instructions,
    );
    if request.is_empty() {
        return reserve_tokens.saturating_add(1);
    }
    let system_prompt_tokens = (super::compaction_utils::SUMMARIZATION_SYSTEM_PROMPT
        .chars()
        .count() as u64)
        .div_ceil(4);
    let prompt_tokens = super::compact_session::summarizer_request_tokens(&request);
    (system_prompt_tokens + prompt_tokens + BRANCH_SUMMARY_MAX_TOKENS)
        .max(prompt_tokens.saturating_add(reserve_tokens))
}

/// Generate the abandoned-branch summary: prepare the entries under the
/// context budget, run the summarizer, and fold in the file-ops block.
pub async fn generate_branch_summary(
    entries: &[FileEntry],
    options: GenerateBranchSummaryOptions<'_>,
) -> BranchSummaryResult {
    let GenerateBranchSummaryOptions {
        model,
        api_key,
        custom_instructions,
        replace_instructions,
        reserve_tokens,
        auxiliary,
        on_model_fallback,
    } = options;
    // The branch summary runs with its own prompt prefix, so on the session model it
    // re-reads the whole branch — route it to the auxiliary model when its known window fits.
    // The fit check estimates the request the SESSION model would issue; the
    // routed model re-slices with its own window. The resolution reads
    // settings/models/auth, so it runs on the blocking pool.
    let (routed, session_headers) = match auxiliary {
        Some(context) => {
            let required = estimate_branch_summary_request_tokens(
                entries,
                model.context_window,
                reserve_tokens,
                custom_instructions,
                replace_instructions,
            );
            let join = {
                let context = context.clone();
                let session_model = model.clone();
                let session_api_key = api_key.clone();
                tokio::task::spawn_blocking(move || {
                    let routed = super::auxiliary_model::resolve_auxiliary_model(
                        &context,
                        "branch summary",
                        &session_model,
                        session_api_key.as_deref(),
                        Some(required),
                    );
                    // The wire fallback's session headers, resolved on the
                    // same blocking hop (a registry read) and only when the
                    // routing actually moved off the session model.
                    let session_headers = if super::compaction_exec::is_session_model(
                        &routed.model,
                        &session_model,
                    ) {
                        None
                    } else {
                        super::auxiliary_model::session_fallback_with_headers(
                            &context,
                            &session_model,
                            session_api_key,
                        )
                        .headers
                    };
                    (routed, session_headers)
                })
            };
            // A JoinError (the closure panicked) degrades to the session
            // fallback, which keeps the merged headers.
            join.await.unwrap_or_else(|_| {
                (
                    super::auxiliary_model::session_fallback_with_headers(
                        context,
                        model,
                        api_key.clone(),
                    ),
                    None,
                )
            })
        }
        None => (
            super::auxiliary_model::ResolvedAuxiliaryModel {
                model: model.clone(),
                api_key: api_key.clone(),
                headers: None,
            },
            None,
        ),
    };
    let request = BranchSummaryRequest {
        entries,
        reserve_tokens,
        custom_instructions,
        replace_instructions,
    };
    let (routed_result, routed_preparation) = match request
        .call(
            &routed.model,
            routed.api_key.clone(),
            routed.headers.clone(),
        )
        .await
    {
        BranchSummaryCall::Empty(preparation) => return no_content_summary(&preparation),
        BranchSummaryCall::Sent(result, preparation) => (result, preparation),
    };
    let routed_failed = !matches!(
        routed_result.as_ref(),
        Ok(response) if response.stop_reason != pa_types::ai::StopReason::Error
    );
    // The routed call failed: retry once on the session model when the
    // routing actually moved (codex `compact_model_fallback.rs`). The retry
    // re-slices the branch for the session model's own window — the routed
    // request was sized for the routed model's.
    let (result, preparation, served_by) = if routed_failed
        && !super::compaction_exec::is_session_model(&routed.model, model)
    {
        eprintln!(
                "Warning: auxiliary model \"{}/{}\" failed for the branch summary; using the session model.",
                routed.model.provider,
                routed.model.id
            );
        match request.call(model, api_key, session_headers).await {
            BranchSummaryCall::Empty(preparation) => return no_content_summary(&preparation),
            BranchSummaryCall::Sent(result, preparation) => {
                if let Some(on_fallback) = &on_model_fallback {
                    on_fallback(matches!(
                        result.as_ref(),
                        Ok(response) if response.stop_reason != pa_types::ai::StopReason::Error
                    ));
                }
                (result, preparation, model)
            }
        }
    } else {
        (routed_result, routed_preparation, &routed.model)
    };
    let response = match *result {
        Ok(response) => response,
        Err(error) => {
            return BranchSummaryResult {
                error: Some(format!("{error:#}")),
                ..Default::default()
            }
        }
    };
    if response.stop_reason == pa_types::ai::StopReason::Aborted {
        return BranchSummaryResult {
            aborted: true,
            ..Default::default()
        };
    }
    if response.stop_reason == pa_types::ai::StopReason::Error {
        return BranchSummaryResult {
            error: Some(
                response
                    .error_message
                    .unwrap_or_else(|| "Summarization failed".to_string()),
            ),
            ..Default::default()
        };
    }
    let response_text = response
        .content
        .iter()
        .filter_map(|block| match block {
            pa_types::ai::AssistantContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut result = finalize_branch_summary(&response_text, &preparation);
    result.usage = (response.usage.total_tokens > 0).then_some(response.usage);
    result.model = Some((served_by.provider.clone(), served_by.id.clone()));
    result
}

/// The inputs of one branch-summary wire call, shared by the routed call and
/// its session-model fallback.
struct BranchSummaryRequest<'a> {
    entries: &'a [FileEntry],
    reserve_tokens: u64,
    custom_instructions: Option<&'a str>,
    replace_instructions: bool,
}

/// One branch-summary call's outcome, with the preparation its request was
/// built from (the file lists fold from it).
enum BranchSummaryCall {
    /// Nothing to summarize under this model's window: no request was sent.
    Empty(BranchPreparation),
    /// The call's outcome, boxed: an assistant message dwarfs the empty
    /// arm.
    Sent(
        Box<Result<pa_types::ai::AssistantMessage, pa_ai::ProviderError>>,
        BranchPreparation,
    ),
}

impl BranchSummaryRequest<'_> {
    /// Slice the branch for `model`'s own window and send the summary
    /// request. One-off branch summaries never read the prompt cache back,
    /// so they opt out entirely (pi `completeSummarization` parity): no cache
    /// writes, and a fresh routing id keeps the session's cache namespace
    /// clean.
    async fn call(
        &self,
        model: &pa_types::ai::Model,
        api_key: Option<String>,
        headers: Option<std::collections::BTreeMap<String, String>>,
    ) -> BranchSummaryCall {
        let context_window = if model.context_window > 0 {
            model.context_window
        } else {
            128_000
        };
        let token_budget = context_window.saturating_sub(self.reserve_tokens);
        let (request_messages, preparation) = build_branch_summary_request(
            self.entries,
            token_budget,
            self.custom_instructions,
            self.replace_instructions,
        );
        if request_messages.is_empty() {
            return BranchSummaryCall::Empty(preparation);
        }
        let context = pa_types::ai::Context {
            system_prompt: Some(super::compaction_utils::SUMMARIZATION_SYSTEM_PROMPT.to_string()),
            messages: convert_to_llm(&request_messages)
                .into_iter()
                .filter_map(|message| match message {
                    AgentMessage::User(user) => {
                        Some(pa_types::ai::Message::User(pa_types::ai::UserMessage {
                            content: user.content,
                            timestamp: user.timestamp,
                            rest: user.rest,
                        }))
                    }
                    _ => None,
                })
                .collect(),
            tools: None,
        };
        let stream_options =
            pa_ai::types::SimpleStreamOptions::from_base(pa_ai::types::StreamOptions {
                max_tokens: Some(BRANCH_SUMMARY_MAX_TOKENS),
                api_key,
                headers: headers.map(|headers| headers.into_iter().collect()),
                cache_retention: Some(pa_types::ai::CacheRetention::None),
                session_id: Some(uuid::Uuid::new_v4().to_string()),
                ..Default::default()
            });
        let result = pa_ai::complete_simple(model, &context, Some(stream_options)).await;
        BranchSummaryCall::Sent(Box::new(result), preparation)
    }
}

/// The result for a branch with nothing to summarize.
fn no_content_summary(preparation: &BranchPreparation) -> BranchSummaryResult {
    let (read_files, modified_files) = compute_file_lists(&preparation.file_ops);
    BranchSummaryResult {
        summary: Some("No content to summarize".to_string()),
        read_files,
        modified_files,
        ..Default::default()
    }
}

/// The presentation message users see for a branch summary.
#[must_use]
pub fn branch_summary_presentation(summary: &str) -> String {
    format!("{BRANCH_SUMMARY_PREFIX}{summary}{BRANCH_SUMMARY_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_types::session::EntryBase;

    fn entry(id: &str, parent: Option<&str>, message: AgentMessage) -> FileEntry {
        FileEntry::Message {
            message,
            base: EntryBase {
                id: Some(id.to_string()),
                parent_id: parent.map(str::to_string),
                timestamp: Some("2024-01-01T00:00:00.000Z".to_string()),
                rest: serde_json::Map::default(),
            },
        }
    }

    fn user(text: &str) -> AgentMessage {
        AgentMessage::User(pa_types::ai::UserMessage {
            content: pa_types::ai::UserContent::Text(text.to_string()),
            timestamp: 0,
            rest: serde_json::Map::default(),
        })
    }

    #[test]
    fn collects_old_branch_to_common_ancestor() {
        // root -> u1 -> a1 -> u2 (old leaf)
        //        \\-> u3 (target)
        let entries = vec![
            entry("u1", None, user("start")),
            entry("a1", Some("u1"), user("assistant turn")),
            entry("u2", Some("a1"), user("old leaf")),
            entry("u3", Some("u1"), user("target")),
        ];
        let result = collect_entries_for_branch_summary(&entries, Some("u2"), "u3");
        assert_eq!(result.common_ancestor_id.as_deref(), Some("u1"));
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].id(), Some("a1"));
        assert_eq!(result.entries[1].id(), Some("u2"));
    }

    #[test]
    fn a_parent_id_cycle_ends_the_walk() {
        // a1 <-> u2 is a cycle with no root; the target sits off it.
        let entries = vec![
            entry("a1", Some("u2"), user("assistant turn")),
            entry("u2", Some("a1"), user("old leaf")),
            entry("u3", None, user("target")),
        ];
        let result = collect_entries_for_branch_summary(&entries, Some("u2"), "u3");
        assert_eq!(result.common_ancestor_id, None);
        assert_eq!(
            result.entries.iter().map(FileEntry::id).collect::<Vec<_>>(),
            vec![Some("a1"), Some("u2")]
        );
    }

    #[test]
    fn preparation_respects_budget_and_keeps_boundaries() {
        let mut entries = Vec::new();
        for i in 0..10 {
            entries.push(entry(
                &format!("e{i}"),
                None,
                user(&format!("message {i} with padding")),
            ));
        }
        // Each message estimates to ~6 tokens; a 6-token budget keeps
        // only the newest one.
        let preparation = prepare_branch_entries(&entries, 6);
        assert_eq!(preparation.messages.len(), 1);
        let tight = prepare_branch_entries(&entries, 1);
        assert_eq!(tight.messages.len(), 0);
        let unlimited = prepare_branch_entries(&entries, 0);
        assert_eq!(unlimited.messages.len(), 10);
    }

    #[test]
    fn request_and_finalize() {
        let entries = vec![entry("e0", None, user("explore the widget"))];
        let (messages, preparation) =
            build_branch_summary_request(&entries, 0, Some("focus on x"), false);
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            AgentMessage::User(user) => {
                let text = user.content.text();
                assert!(text.starts_with("<conversation>"));
                assert!(text.contains("Additional focus: focus on x"));
            }
            _ => panic!("expected user"),
        }
        let result = finalize_branch_summary("## Goal\nexplore", &preparation);
        let summary = result.summary.unwrap();
        assert!(summary.contains("The user explored a different conversation branch"));
        assert!(summary.contains("## Goal"));
        assert!(branch_summary_presentation("s").contains("[branch-summary]"));
    }

    #[test]
    fn compaction_entries_become_summary_messages() {
        let compaction = FileEntry::Compaction {
            payload: pa_types::session::CompactionEntry {
                summary: "prior state".to_string(),
                first_kept_entry_id: "x".to_string(),
                tokens_before: 10,
                details: None,
                from_hook: None,
                custom_instructions: None,
                usage: None,
                harness_digest: None,
                harness_state_fingerprint: None,
                structural_state: None,
            },
            base: EntryBase {
                id: Some("c0".to_string()),
                parent_id: None,
                timestamp: Some("2024-01-01T00:00:00.000Z".to_string()),
                rest: serde_json::Map::default(),
            },
        };
        let message = get_message_from_entry(&compaction).unwrap();
        match message {
            AgentMessage::CompactionSummary(summary) => {
                assert_eq!(summary.summary, "prior state");
                assert_eq!(summary.tokens_before, 10);
            }
            _ => panic!("expected compaction summary"),
        }
    }

    #[tokio::test]
    async fn generates_summary_through_the_faux_provider() {
        // One faux provider per test (a process-global registry); the
        // guard is released before the awaited model call.
        static FAUX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let registration = {
            let _guard = FAUX_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions::default())
        };
        let model = registration.get_model();
        let response = pa_ai::faux::faux_assistant_text_message(
            "## Goal\nexplore the tree",
            pa_ai::faux::FauxAssistantMessageOptions::default(),
        );
        registration.set_responses(vec![pa_ai::faux::FauxResponseStep::Message(response)]);
        let entries = vec![entry("e0", None, user("explore the widget"))];
        let result = generate_branch_summary(
            &entries,
            GenerateBranchSummaryOptions {
                model: &model,
                api_key: None,
                custom_instructions: Some("focus on x"),
                replace_instructions: false,
                reserve_tokens: DEFAULT_BRANCH_RESERVE_TOKENS,
                auxiliary: None,
                on_model_fallback: None,
            },
        )
        .await;
        assert!(!result.aborted, "not aborted");
        assert!(result.error.is_none(), "error: {:?}", result.error);
        let summary = result.summary.expect("summary text");
        assert!(
            summary.starts_with("The user explored a different conversation branch"),
            "preamble present: {summary}"
        );
        assert!(summary.contains("## Goal"));
        registration.unregister();
    }

    #[tokio::test]
    async fn empty_branch_summarizes_to_a_note() {
        let model: pa_types::ai::Model = serde_json::from_value(serde_json::json!({
            "id": "m", "name": "m", "api": "openai-completions", "provider": "test",
            "baseUrl": "http://localhost", "reasoning": false, "input": ["text"],
            "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
            "contextWindow": 1000, "maxTokens": 100
        }))
        .unwrap();
        let result = generate_branch_summary(
            &[],
            GenerateBranchSummaryOptions {
                model: &model,
                api_key: None,
                custom_instructions: None,
                replace_instructions: false,
                reserve_tokens: DEFAULT_BRANCH_RESERVE_TOKENS,
                auxiliary: None,
                on_model_fallback: None,
            },
        )
        .await;
        assert_eq!(result.summary.as_deref(), Some("No content to summarize"));
        assert!(!result.aborted);
        assert!(result.error.is_none());
    }

    /// A selector equal to the session model keeps it, and a selector
    /// that resolves to no model falls back the same way.
    #[tokio::test]
    async fn branch_summary_auxiliary_selector_falls_back_to_the_session_model() {
        static FAUX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        for selector in ["faux/faux-1", "testaux/missing-model"] {
            let registration = {
                let _guard = FAUX_LOCK
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                pa_ai::faux::register_faux_provider(
                    pa_ai::faux::RegisterFauxProviderOptions::default(),
                )
            };
            let model = registration.get_model();
            let tmp = tempfile::tempdir().unwrap();
            std::fs::write(
                tmp.path().join("settings.json"),
                serde_json::to_string_pretty(&serde_json::json!({
                    "auxiliaryModel": selector,
                }))
                .unwrap(),
            )
            .unwrap();
            let aux = super::super::auxiliary_model::AuxiliaryModelContext {
                cwd: tmp.path().to_path_buf(),
                agent_dir: tmp.path().to_path_buf(),
            };
            let seen_models: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
                std::sync::Arc::default();
            let recorder = seen_models.clone();
            registration.set_responses(vec![pa_ai::faux::FauxResponseStep::Factory(
                std::sync::Arc::new(
                    move |_context: &pa_types::ai::Context,
                          _options: Option<&pa_ai::types::StreamOptions>,
                          _call: u64,
                          model: &pa_types::ai::Model| {
                        recorder.lock().unwrap().push(model.id.clone());
                        Ok(pa_ai::faux::faux_assistant_text_message(
                            "## Goal\nexplore the tree",
                            pa_ai::faux::FauxAssistantMessageOptions::default(),
                        ))
                    },
                ),
            )]);
            let entries = vec![entry("e0", None, user("explore the widget"))];
            let result = generate_branch_summary(
                &entries,
                GenerateBranchSummaryOptions {
                    model: &model,
                    api_key: None,
                    custom_instructions: None,
                    replace_instructions: false,
                    reserve_tokens: DEFAULT_BRANCH_RESERVE_TOKENS,
                    auxiliary: Some(&aux),
                    on_model_fallback: None,
                },
            )
            .await;
            assert!(result.error.is_none(), "error: {:?}", result.error);
            assert!(result.summary.is_some());
            assert_eq!(seen_models.lock().unwrap().as_slice(), ["faux-1"]);
            registration.unregister();
        }
    }

    /// The window estimate mirrors `generate_branch_summary`'s slicing
    /// and prompt exactly.
    #[test]
    fn branch_window_estimate_shapes() {
        // An entry too large for the slice budget: no wire request, but
        // the estimate still requires a window above the reserve.
        let big = vec![entry("e0", None, user(&"x".repeat(100_000)))];
        assert_eq!(
            estimate_branch_summary_request_tokens(
                &big,
                20_000,
                DEFAULT_BRANCH_RESERVE_TOKENS,
                None,
                false
            ),
            DEFAULT_BRANCH_RESERVE_TOKENS + 1
        );
        // The larger of the request body and the reserve draw wins, so
        // a bigger reserve grows the estimate.
        let entries = vec![entry("e0", None, user("explore the widget"))];
        let big_reserve = estimate_branch_summary_request_tokens(
            &entries,
            200_000,
            DEFAULT_BRANCH_RESERVE_TOKENS,
            Some("focus on x"),
            false,
        );
        let small_reserve = estimate_branch_summary_request_tokens(
            &entries,
            200_000,
            100,
            Some("focus on x"),
            false,
        );
        assert!(big_reserve > small_reserve);
        let wide = vec![
            entry("e0", None, user(&"y".repeat(400_000))),
            entry("e1", Some("e0"), user(&"y".repeat(400_000))),
            entry("e2", Some("e1"), user(&"y".repeat(400_000))),
        ];
        let big_window =
            estimate_branch_summary_request_tokens(&wide, 200_000, 16_384, None, false);
        let small_window =
            estimate_branch_summary_request_tokens(&wide, 100_000, 16_384, None, false);
        assert!(big_window > small_window);
    }

    /// A routed auxiliary model that fails on the wire falls back to the
    /// session model, and the retry re-slices the branch for the session
    /// model's own (smaller) window instead of resending the routed request.
    #[tokio::test]
    async fn branch_summary_wire_failure_retries_on_the_session_model_resliced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{ "auxiliaryModel": "testaux/aux-model" }"#,
        )
        .unwrap();
        // The auxiliary provider listens nowhere (port 9): its call fails on
        // the wire.
        std::fs::write(
            dir.path().join("models.json"),
            r#"{ "providers": { "testaux": {
                "baseUrl": "http://127.0.0.1:9", "apiKey": "aux-key",
                "api": "openai-completions",
                "models": [ { "id": "aux-model", "name": "Aux", "contextWindow": 128000 } ]
            } } }"#,
        )
        .unwrap();
        let auxiliary = super::super::auxiliary_model::AuxiliaryModelContext {
            cwd: dir.path().to_path_buf(),
            agent_dir: dir.path().to_path_buf(),
        };
        let registration =
            pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
                models: Some(vec![pa_ai::faux::FauxModelDefinition {
                    id: "branch-session-m".to_string(),
                    name: Some("Branch Session Model".to_string()),
                    reasoning: Some(false),
                    input: Some(vec![pa_types::ai::ModelInput::Text]),
                    cost: None,
                    context_window: Some(1_500),
                    max_tokens: Some(256),
                }]),
                ..Default::default()
            });
        let model = registration.get_model();
        let prompts: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::default();
        let recorder = prompts.clone();
        registration.set_responses(vec![pa_ai::faux::FauxResponseStep::Factory(
            std::sync::Arc::new(
                move |context: &pa_types::ai::Context,
                      _options: Option<&pa_ai::types::StreamOptions>,
                      _call: u64,
                      _model: &pa_types::ai::Model| {
                    let text = context
                        .messages
                        .iter()
                        .map(|message| match message {
                            pa_types::ai::Message::User(user) => user.content.text(),
                            _ => String::new(),
                        })
                        .collect::<String>();
                    recorder.lock().unwrap().push(text);
                    Ok(pa_ai::faux::faux_assistant_text_message(
                        "## Goal\nthe fallback summary",
                        pa_ai::faux::FauxAssistantMessageOptions::default(),
                    ))
                },
            ),
        )]);
        // Twelve ~1,000-char turns: the routed 128k window takes them all,
        // the session's 1,500-token window only the newest few.
        let mut entries = Vec::new();
        for index in 0..12 {
            let parent = (index > 0).then(|| format!("u{}", index - 1));
            entries.push(entry(
                &format!("u{index}"),
                parent.as_deref(),
                user(&format!("turn-marker-{index:02} {}", "x".repeat(1_000))),
            ));
        }
        let reported: std::sync::Arc<std::sync::Mutex<Vec<bool>>> = std::sync::Arc::default();
        let report = reported.clone();
        let result = generate_branch_summary(
            &entries,
            GenerateBranchSummaryOptions {
                model: &model,
                api_key: None,
                custom_instructions: None,
                replace_instructions: false,
                reserve_tokens: 500,
                auxiliary: Some(&auxiliary),
                on_model_fallback: Some(std::sync::Arc::new(move |succeeded| {
                    report.lock().unwrap().push(succeeded);
                })),
            },
        )
        .await;
        assert_eq!(result.error, None);
        assert!(result
            .summary
            .as_deref()
            .is_some_and(|summary| summary.contains("the fallback summary")));
        assert_eq!(
            result.model,
            Some((model.provider.clone(), model.id.clone())),
            "the session model served the summary"
        );
        assert_eq!(reported.lock().unwrap().as_slice(), [true]);
        let prompts = prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1, "one session-model call");
        assert!(
            prompts[0].contains("turn-marker-11"),
            "the newest turn is kept"
        );
        assert!(
            !prompts[0].contains("turn-marker-00"),
            "the retry was re-sliced for the session window"
        );
        registration.unregister();
    }

    /// The one-off branch summary opts out of prompt caching, like the
    /// compaction summarizer: `CacheRetention::None` plus a fresh routing id.
    #[tokio::test]
    async fn branch_summary_opts_out_of_prompt_caching() {
        type Captured = (Option<pa_types::ai::CacheRetention>, Option<String>);
        let registration =
            pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
                models: Some(vec![pa_ai::faux::FauxModelDefinition {
                    id: "branch-m".to_string(),
                    name: Some("Branch Model".to_string()),
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
                        "the branch summary",
                        pa_ai::faux::FauxAssistantMessageOptions::default(),
                    ))
                },
            ),
        )]);
        let entries = vec![
            entry("u1", None, user("start")),
            entry("a1", Some("u1"), user("assistant turn")),
            entry("u2", Some("a1"), user("the old leaf")),
        ];
        let result = generate_branch_summary(
            &entries,
            GenerateBranchSummaryOptions {
                model: &model,
                api_key: None,
                custom_instructions: None,
                replace_instructions: false,
                reserve_tokens: 100,
                auxiliary: None,
                on_model_fallback: None,
            },
        )
        .await;
        assert!(result.summary.is_some());
        let seen = seen.lock().unwrap();
        let (retention, session_id) = seen[0].clone();
        assert_eq!(retention, Some(pa_types::ai::CacheRetention::None));
        assert!(session_id.is_some(), "one-off requests route on a fresh id");
        registration.unregister();
    }
}
