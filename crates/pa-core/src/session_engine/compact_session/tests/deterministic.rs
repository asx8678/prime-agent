//! Compact-session tests, the deterministic family: the engine that builds
//! the summary from the session log without a model call — the saved state
//! the next compaction folds further, the switches between engines, the
//! active branch as the only source, the size bound on small windows, and
//! the ledger frame of a compaction with no summary call.
use super::*;

use serde_json::json;

use crate::session_engine::semantic_edges::CompactionStatus;
use crate::settings::CompactionEngine;

/// A deterministic compaction over `model` that keeps about one turn.
fn deterministic(model: pa_types::ai::Model) -> CompactOptions<'static> {
    CompactOptions {
        model,
        api_key: None,
        custom_instructions: None,
        preserve: Vec::new(),
        carried: Vec::new(),
        settings: super::super::compaction::CompactionSettings {
            keep_recent_tokens: 10,
            engine: CompactionEngine::Deterministic,
            ..Default::default()
        },
        abort: None,
        harness_digest: None,
        auxiliary: None,
        summary_delta: None,
        semantic_edges: None,
        on_model_fallback: None,
    }
}

/// The faux model with a window that leaves the summary its whole budget.
fn wide_model(registration: &pa_ai::faux::FauxProviderRegistration) -> pa_types::ai::Model {
    let mut model = registration.get_model();
    model.context_window = 200_000;
    model
}

fn ran(outcome: CompactOutcome) -> CompactRun {
    match outcome {
        CompactOutcome::Ran(run) => *run,
        CompactOutcome::Skipped(reason) => panic!("the compaction skipped: {reason}"),
    }
}

fn users_folded(run: &CompactRun) -> serde_json::Value {
    run.entry
        .structural_state
        .as_ref()
        .expect("the row saves the folded state")["users"]
        .clone()
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::User(pa_types::ai::UserMessage {
        content: UserContent::Text(text.to_string()),
        timestamp: 0,
        rest: serde_json::Map::default(),
    })
}

fn reply(text: &str) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![pa_types::ai::AssistantContentBlock::Text(
            pa_types::ai::TextContent {
                text: text.to_string(),
                text_signature: None,
                rest: serde_json::Map::default(),
            },
        )],
        api: "faux".to_string(),
        provider: "faux".to_string(),
        model: "compact-m".to_string(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: pa_types::ai::Usage::default(),
        stop_reason: pa_types::ai::StopReason::Stop,
        stop_reason_raw: None,
        error_message: None,
        timestamp: 0,
        rest: serde_json::Map::default(),
    })
}

/// No summarizer call (the script is empty, so a call would fail the run),
/// no usage, the folded state on the row, and the live block receives the
/// committed summary in one piece.
#[tokio::test]
async fn a_deterministic_compaction_makes_no_model_call_and_streams_its_summary_whole() {
    let registration = faux_registration();
    registration.set_responses(Vec::new());
    let tmp = tempfile::tempdir().unwrap();
    let mut session = session_with_turns(tmp.path(), 3);
    let deltas: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::default();
    let sink_deltas = deltas.clone();
    let run = ran(execute_compaction(
        &mut session,
        CompactOptions {
            summary_delta: Some(std::sync::Arc::new(move |delta: &str| {
                sink_deltas.lock().unwrap().push(delta.to_string());
            })),
            ..deterministic(wide_model(&registration))
        },
    )
    .await
    .unwrap());
    assert_eq!(registration.call_count(), 0);
    assert_eq!(
        *deltas.lock().unwrap(),
        std::slice::from_ref(&run.result.summary)
    );
    assert!(
        run.result.summary.starts_with("[Deterministic summary]"),
        "{}",
        run.result.summary
    );
    assert!(run
        .result
        .summary
        .contains("turn 0 message with some words"));
    assert_eq!(run.result.usage, None);
    assert_eq!(users_folded(&run), json!(2));
    // The row keeps the model summarizer's record, so switching back still
    // inherits the file lists.
    assert_eq!(
        (
            &run.entry.summary,
            run.entry.from_hook,
            &run.entry.details,
            run.entry.usage
        ),
        (
            &run.result.summary,
            Some(false),
            &Some(json!({ "readFiles": [], "modifiedFiles": [] })),
            None
        )
    );
    registration.unregister();
}

/// The second compaction folds the first one's saved state further: the
/// earlier facts come from the state, never from re-reading its text.
#[tokio::test]
async fn a_second_deterministic_compaction_folds_the_saved_state_further() {
    let registration = faux_registration();
    registration.set_responses(Vec::new());
    let model = wide_model(&registration);
    let tmp = tempfile::tempdir().unwrap();
    let mut session = session_with_turns(tmp.path(), 3);
    let first = ran(
        execute_compaction(&mut session, deterministic(model.clone()))
            .await
            .unwrap(),
    );
    append_turn(&mut session, 3);
    append_turn(&mut session, 4);
    let second = ran(execute_compaction(&mut session, deterministic(model))
        .await
        .unwrap());
    assert_eq!(
        (users_folded(&first), users_folded(&second)),
        (json!(2), json!(4))
    );
    assert!(second
        .result
        .summary
        .contains("turn 0 message with some words"));
    assert!(second
        .result
        .summary
        .contains("turn 3 message with some words"));
    assert!(!second.result.summary.contains("[Earlier summaries]"));
    assert_eq!(registration.call_count(), 0);
    registration.unregister();
}

/// A session the model summarizer compacted first keeps that summary once,
/// as a labeled excerpt.
#[tokio::test]
async fn switching_from_the_model_summarizer_keeps_its_summary_as_a_labeled_excerpt() {
    let registration = faux_registration();
    let tmp = tempfile::tempdir().unwrap();
    let mut session = session_with_turns(tmp.path(), 3);
    let first = ran(execute_compaction(
        &mut session,
        CompactOptions {
            settings: super::super::compaction::CompactionSettings {
                keep_recent_tokens: 10,
                engine: CompactionEngine::Llm,
                ..Default::default()
            },
            ..deterministic(registration.get_model())
        },
    )
    .await
    .unwrap());
    assert_eq!(first.entry.structural_state, None);
    append_turn(&mut session, 3);
    append_turn(&mut session, 4);
    let second = ran(
        execute_compaction(&mut session, deterministic(wide_model(&registration)))
            .await
            .unwrap(),
    );
    assert_eq!(
        registration.call_count(),
        1,
        "only the first run called the model"
    );
    assert!(
        second.result.summary.contains(
            "[Earlier summaries]\nEarlier compaction summary, model-written [u0]:\n## Goal\nsummarized goal"
        ),
        "{}",
        second.result.summary
    );
    registration.unregister();
}

/// A note pinned before a model-summarizer compaction (which saves no state)
/// still renders when the deterministic engine compacts next: the live carry
/// list on the branch is the source, not the fold window.
#[tokio::test]
async fn a_carried_note_survives_a_model_summarizer_compaction_in_between() {
    let registration = faux_registration();
    let tmp = tempfile::tempdir().unwrap();
    let mut session = SessionManager::in_memory(tmp.path());
    let items = vec!["keep the API stable".to_string()];
    session
        .append_custom_message(
            crate::session_engine::compact_carry::COMPACT_CARRY_CUSTOM_TYPE,
            UserContent::Text(crate::session_engine::compact_carry::carry_row_content(
                &items,
            )),
            false,
            Some(crate::session_engine::compact_carry::carry_row_details(
                &items,
            )),
        )
        .unwrap();
    for i in 0..3 {
        append_turn(&mut session, i);
    }
    let first = ran(execute_compaction(
        &mut session,
        CompactOptions {
            settings: super::super::compaction::CompactionSettings {
                keep_recent_tokens: 10,
                engine: CompactionEngine::Llm,
                ..Default::default()
            },
            ..deterministic(registration.get_model())
        },
    )
    .await
    .unwrap());
    assert_eq!(first.entry.structural_state, None);
    append_turn(&mut session, 3);
    append_turn(&mut session, 4);
    let second = ran(
        execute_compaction(&mut session, deterministic(wide_model(&registration)))
            .await
            .unwrap(),
    );
    assert!(
        second
            .result
            .summary
            .contains("[Carried notes]\n\n- keep the API stable [carry:0]"),
        "{}",
        second.result.summary
    );
    registration.unregister();
}

/// Switching back hands the model summarizer the deterministic summary as
/// the previous summary to update.
#[tokio::test]
async fn switching_back_to_the_model_summarizer_updates_the_deterministic_summary() {
    let registration = faux_registration();
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = std::sync::Arc::default();
    let seen_requests = seen.clone();
    registration.set_responses(vec![pa_ai::faux::FauxResponseStep::Factory(
        std::sync::Arc::new(
            move |context: &pa_types::ai::Context,
                  _options: Option<&pa_ai::types::StreamOptions>,
                  _call: u64,
                  _model: &pa_types::ai::Model| {
                let pa_types::ai::Message::User(request) = &context.messages[0] else {
                    panic!("expected a user request")
                };
                seen_requests.lock().unwrap().push(request.content.text());
                Ok(pa_ai::faux::faux_assistant_text_message(
                    "the model summary",
                    pa_ai::faux::FauxAssistantMessageOptions::default(),
                ))
            },
        ),
    )]);
    let tmp = tempfile::tempdir().unwrap();
    let mut session = session_with_turns(tmp.path(), 3);
    let first = ran(
        execute_compaction(&mut session, deterministic(wide_model(&registration)))
            .await
            .unwrap(),
    );
    append_turn(&mut session, 3);
    append_turn(&mut session, 4);
    let second = ran(execute_compaction(
        &mut session,
        CompactOptions {
            settings: super::super::compaction::CompactionSettings {
                keep_recent_tokens: 10,
                engine: CompactionEngine::Llm,
                ..Default::default()
            },
            ..deterministic(registration.get_model())
        },
    )
    .await
    .unwrap());
    assert_eq!(second.result.summary, "the model summary");
    assert_eq!(second.entry.structural_state, None);
    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].contains(&format!(
            "<previous-summary>\n{}\n</previous-summary>",
            first.result.summary.trim_end()
        )),
        "{}",
        requests[0]
    );
    registration.unregister();
}

/// A branch the session moved away from is never summarized: the
/// in-memory rows still hold it, but only the active path is folded.
#[tokio::test]
async fn an_abandoned_branch_never_reaches_the_deterministic_summary() {
    let registration = faux_registration();
    registration.set_responses(Vec::new());
    let tmp = tempfile::tempdir().unwrap();
    let mut session = SessionManager::in_memory(tmp.path());
    session.append_message(user("first request")).unwrap();
    let branch_point = session.append_message(reply("first reply")).unwrap();
    session.append_message(user("abandoned plan")).unwrap();
    session.append_message(reply("abandoned reply")).unwrap();
    session.branch(&branch_point);
    session.append_message(user("second request")).unwrap();
    session.append_message(reply("second reply")).unwrap();
    session.append_message(user("third request")).unwrap();
    session.append_message(reply("third reply")).unwrap();
    let run = ran(
        execute_compaction(&mut session, deterministic(wide_model(&registration)))
            .await
            .unwrap(),
    );
    assert!(run.result.summary.contains("second request"));
    assert!(
        !run.result.summary.contains("abandoned"),
        "{}",
        run.result.summary
    );
    assert_eq!(users_folded(&run), json!(2));
    registration.unregister();
}

/// Compacting on a branch and then branching back leaves the abandoned rows
/// — and the compaction that branch ended in — out of the next fold: it
/// starts from the active branch alone, so no abandoned fact reappears and
/// the user numbering restarts at the branch point.
#[tokio::test]
async fn a_compaction_after_a_branch_back_leaves_the_abandoned_branch_behind() {
    let registration = faux_registration();
    registration.set_responses(Vec::new());
    let model = wide_model(&registration);
    let tmp = tempfile::tempdir().unwrap();
    let mut session = SessionManager::in_memory(tmp.path());
    session.append_message(user("first request")).unwrap();
    let branch_point = session.append_message(reply("first reply")).unwrap();
    session.append_message(user("abandoned plan")).unwrap();
    session.append_message(reply("abandoned reply")).unwrap();
    session.append_message(user("abandoned more")).unwrap();
    session
        .append_message(reply("abandoned more reply"))
        .unwrap();
    ran(
        execute_compaction(&mut session, deterministic(model.clone()))
            .await
            .unwrap(),
    );
    session.branch(&branch_point);
    session.append_message(user("second request")).unwrap();
    session.append_message(reply("second reply")).unwrap();
    session.append_message(user("third request")).unwrap();
    session.append_message(reply("third reply")).unwrap();
    let second = ran(execute_compaction(&mut session, deterministic(model))
        .await
        .unwrap());
    assert!(
        !second.result.summary.contains("abandoned"),
        "{}",
        second.result.summary
    );
    assert!(
        second
            .result
            .summary
            .contains("User instruction [u1]:\nfirst request"),
        "{}",
        second.result.summary
    );
    assert!(
        second
            .result
            .summary
            .contains("User instruction [u2]:\nsecond request"),
        "{}",
        second.result.summary
    );
    assert_eq!(users_folded(&second), json!(2));
    registration.unregister();
}

/// A long abandoned branch with a short active tail never moves the kept
/// boundary off the active path: the branch-only cut either keeps an
/// on-branch row or the compaction skips — the abandoned rows alone are
/// never enough history to cut into.
#[tokio::test]
async fn a_long_abandoned_branch_never_cuts_the_kept_boundary_off_the_active_path() {
    let registration = faux_registration();
    registration.set_responses(Vec::new());
    let tmp = tempfile::tempdir().unwrap();
    let mut session = SessionManager::in_memory(tmp.path());
    session.append_message(user("root request")).unwrap();
    let branch_point = session.append_message(reply("root reply")).unwrap();
    for turn in 0..8 {
        session
            .append_message(user(&format!(
                "abandoned request {turn} {}",
                "x".repeat(400)
            )))
            .unwrap();
        session
            .append_message(reply(&format!(
                "abandoned reply {turn} {}",
                "y".repeat(400)
            )))
            .unwrap();
    }
    session.branch(&branch_point);
    session.append_message(user("tail request")).unwrap();
    session.append_message(reply("tail reply")).unwrap();
    let active_ids: std::collections::HashSet<String> = session
        .active_branch_entries()
        .into_iter()
        .filter_map(|entry| entry.id().map(str::to_string))
        .collect();
    let outcome = execute_compaction(
        &mut session,
        CompactOptions {
            settings: super::super::compaction::CompactionSettings {
                keep_recent_tokens: 50,
                engine: CompactionEngine::Deterministic,
                ..Default::default()
            },
            ..deterministic(wide_model(&registration))
        },
    )
    .await
    .unwrap();
    match outcome {
        CompactOutcome::Skipped(_) => {}
        CompactOutcome::Ran(run) => assert!(
            active_ids.contains(&run.result.first_kept_entry_id),
            "the cut kept an off-branch row: {}",
            run.result.first_kept_entry_id
        ),
    }
    registration.unregister();
}

/// A small context window scales the summary down to half its size in
/// bytes, so the kept messages still fit.
#[tokio::test]
async fn a_small_context_window_scales_the_deterministic_summary_down() {
    let registration = faux_registration();
    registration.set_responses(Vec::new());
    let model = registration.get_model();
    assert_eq!(model.context_window, 1_000);
    let tmp = tempfile::tempdir().unwrap();
    let mut session = SessionManager::in_memory(tmp.path());
    for turn in 0..4 {
        session
            .append_message(user(&format!("request {turn} {}", "x".repeat(2_000))))
            .unwrap();
        session
            .append_message(reply(&format!("reply {turn} {}", "y".repeat(2_000))))
            .unwrap();
    }
    let run = ran(execute_compaction(
        &mut session,
        CompactOptions {
            settings: super::super::compaction::CompactionSettings {
                keep_recent_tokens: 600,
                engine: CompactionEngine::Deterministic,
                ..Default::default()
            },
            ..deterministic(model)
        },
    )
    .await
    .unwrap());
    assert!(run.result.summary.starts_with("[Deterministic summary]"));
    assert!(
        run.result.summary.len() <= 500,
        "{} bytes",
        run.result.summary.len()
    );
    registration.unregister();
}

/// A deterministic summary counts as a previous summary for the skip guard,
/// exactly like the model summarizer's: a session with nothing new to fold
/// still prepares.
#[test]
fn a_deterministic_previous_summary_keeps_the_session_compactable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut session = session_with_turns(tmp.path(), 3);
    session
        .append_compaction(pa_types::session::CompactionEntry {
            summary: "[Deterministic summary] earlier facts".to_string(),
            first_kept_entry_id: session.retained_entries()[1].id().unwrap().to_string(),
            structural_state: Some(json!({ "version": 1 })),
            ..Default::default()
        })
        .unwrap();
    session.append_model_change("faux", "compact-m").unwrap();
    let preparation = prepare_compaction(session.retained_entries(), 10_000).unwrap();
    assert_eq!(
        (
            preparation.previous_summary.as_deref(),
            preparation.previous_state
        ),
        (
            Some("[Deterministic summary] earlier facts"),
            Some(json!({ "version": 1 }))
        )
    );
}

/// A compaction without summary slices still frames its ledger entry: it
/// begins and completes, with no summary request under it.
#[tokio::test]
async fn a_deterministic_compaction_completes_on_the_ledger_without_summary_requests() {
    let registration = faux_registration();
    registration.set_responses(Vec::new());
    let tmp = tempfile::tempdir().unwrap();
    let mut session = session_with_turns(tmp.path(), 3);
    let (identity, recorder) = semantic_fixture(tmp.path());
    ran(execute_compaction(
        &mut session,
        CompactOptions {
            semantic_edges: Some(recorder),
            ..deterministic(wide_model(&registration))
        },
    )
    .await
    .unwrap());
    let events = ledger_events(&identity);
    let compaction_id = begun_compaction_id(&events);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, LedgerEvent::RequestStarted { .. })),
        "{events:?}"
    );
    assert_eq!(
        events.last(),
        Some(&LedgerEvent::CompactionFinished {
            compaction_id,
            status: CompactionStatus::Completed,
        })
    );
    registration.unregister();
}
