//! Compact-session tests.
use super::*;
use pa_types::ai::{AssistantMessage, UserContent};

use crate::session_engine::semantic_edges::{
    SemanticEdgeIdentity, SemanticEdgeLedgerEvent as LedgerEvent, SemanticEdgeRecorder,
    SEMANTIC_EDGES_LEDGER_FILENAME,
};

fn session_with_turns(cwd: &std::path::Path, turns: usize) -> SessionManager {
    let mut session = SessionManager::in_memory(cwd);
    for i in 0..turns {
        append_turn(&mut session, i);
    }
    session
}

/// One user message and its reply, numbered `i`.
fn append_turn(session: &mut SessionManager, i: usize) {
    session
        .append_message(AgentMessage::User(pa_types::ai::UserMessage {
            content: UserContent::Text(format!("turn {i} message with some words")),
            timestamp: 0,
            rest: serde_json::Map::default(),
        }))
        .unwrap();
    session
        .append_message(AgentMessage::Assistant(AssistantMessage {
            content: vec![pa_types::ai::AssistantContentBlock::Text(
                pa_types::ai::TextContent {
                    text: format!("reply {i}"),
                    text_signature: None,
                    rest: serde_json::Map::default(),
                },
            )],
            api: "openai-completions".to_string(),
            provider: "test".to_string(),
            model: "m".to_string(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: pa_types::ai::Usage {
                input: 100,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                total_tokens: 120,
                cost: pa_types::ai::UsageCost::default(),
            },
            stop_reason: pa_types::ai::StopReason::Stop,
            stop_reason_raw: None,
            error_message: None,
            timestamp: 0,
            rest: serde_json::Map::default(),
        }))
        .unwrap();
}

/// The faux provider with one scripted summarizer response. The faux
/// seam is process-global, so every registration unregisters on drop.
fn faux_registration() -> pa_ai::faux::FauxProviderRegistration {
    let registration =
        pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
            models: Some(vec![pa_ai::faux::FauxModelDefinition {
                id: "compact-m".to_string(),
                name: Some("Compact Model".to_string()),
                reasoning: Some(false),
                input: Some(vec![pa_types::ai::ModelInput::Text]),
                cost: None,
                context_window: Some(1_000),
                max_tokens: Some(256),
            }]),
            ..Default::default()
        });
    registration.set_responses(vec![pa_ai::faux::FauxResponseStep::Message(
        pa_ai::faux::faux_assistant_text_message(
            "## Goal\nsummarized goal",
            pa_ai::faux::FauxAssistantMessageOptions::default(),
        ),
    )]);
    registration
}

/// One recorder over a ledger in `dir` (the semantic-edge tests' fixture).
fn semantic_fixture(
    dir: &std::path::Path,
) -> (SemanticEdgeIdentity, std::sync::Arc<SemanticEdgeRecorder>) {
    let identity = SemanticEdgeIdentity {
        session_id: "compact-session".to_string(),
        ledger_path: Some(dir.join(SEMANTIC_EDGES_LEDGER_FILENAME)),
        parent_session_id: None,
        spawned_by_request_id: None,
    };
    let recorder = std::sync::Arc::new(SemanticEdgeRecorder::open(identity.clone()));
    (identity, recorder)
}

fn ledger_events(identity: &SemanticEdgeIdentity) -> Vec<LedgerEvent> {
    let contents = std::fs::read_to_string(identity.ledger_path.as_ref().unwrap()).unwrap();
    contents
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn begun_compaction_id(events: &[LedgerEvent]) -> String {
    events
        .iter()
        .find_map(|event| match event {
            LedgerEvent::CompactionBegun { compaction_id, .. } => Some(compaction_id.clone()),
            _ => None,
        })
        .expect("the compaction began")
}

mod auxiliary_model;
mod cut_math;
mod deterministic;
mod execute_compaction;
mod prepare_compaction;
mod second_compaction;
mod skip_guards;
mod split_turn;

async fn execute_compaction(
    session: &mut SessionManager,
    options: CompactOptions<'_>,
) -> anyhow::Result<CompactOutcome> {
    let started_at = std::time::Instant::now();
    let mut attempt = match prepare_attempt(session, &options) {
        Ok(attempt) => attempt,
        Err(skip) => return Ok(CompactOutcome::Skipped(skip.user_message())),
    };
    let prepared = summarize_attempt(&attempt, &options).await?;
    assert!(
        commit_attempt(session, &mut attempt, &prepared, options.abort)?,
        "no concurrent writer in a unit test"
    );
    Ok(CompactOutcome::Ran(Box::new(CompactRun {
        result: prepared.result,
        entry: prepared.entry,
        duration_ms: started_at.elapsed().as_millis() as u64,
        ipython_state: None,
    })))
}
