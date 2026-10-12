// Pedantic-gate dispositions as src/lib.rs (large_futures/too_many_lines).
#![allow(clippy::large_futures, clippy::too_many_lines)]

//! Verifier: context folding over a REAL kernel. The scripted model opens a
//! fold, runs two work cells, closes it and answers; each fold cell calls
//! the `rlm.host_request` bridge and emits the host's marker exactly as the
//! `fold` skill does (the skill itself stays out of the kernel install).
//! The assertions read what each provider request carried.
#![cfg(unix)]

use std::path::PathBuf;

use pa_agent::scripted::ScriptedProvider;
use pa_core::session_engine::engine::{create_session, SessionEngineConfig};
use pa_core::session_engine::PromptOptions;
use serde_json::{json, Value};

/// The kernel Python with prime-agent-runtime installed. Skipped (with a
/// note) on machines without a live install; `PA_CORE_KERNEL_PYTHON` points
/// at an explicit one.
fn kernel_python() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("PA_CORE_KERNEL_PYTHON") {
        let explicit = PathBuf::from(explicit);
        assert!(
            explicit.exists(),
            "PA_CORE_KERNEL_PYTHON {} not found",
            explicit.display()
        );
        return Some(explicit);
    }
    let candidate = PathBuf::from(std::env::var("HOME").map_or_else(
        |_| "/home/ubuntu/.prime/agent/kernel-venv/bin/python".to_string(),
        |home| format!("{home}/.prime/agent/kernel-venv/bin/python"),
    ));
    if candidate.exists() {
        return Some(candidate);
    }
    eprintln!(
        "kernel python {} not found; skipping live kernel test",
        candidate.display()
    );
    None
}

/// Scoped process-env overrides: applied on construction, restored on drop.
/// This binary carries exactly one live-kernel test, so nothing races.
struct EnvOverride {
    saved: Vec<(String, Option<String>)>,
}

impl EnvOverride {
    fn apply(pairs: &[(&str, Option<String>)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect();
        for (key, value) in pairs {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        EnvOverride { saved }
    }
}

impl Drop for EnvOverride {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

/// A cell that sends one `fold.*` request and handles the answer the way
/// the `fold` skill does: emit the marker when there is one, print the text.
fn fold_cell(request_type: &str, payload: &Value) -> String {
    format!(
        "from rlm import emit, host_request\nanswer = await host_request({request_type:?}, {payload})\nif answer.get(\"marker\") is not None:\n    emit({{\"application/vnd.prime-agent.context-fold+json\": answer[\"marker\"]}})\nprint(answer[\"text\"])"
    )
}

fn scripted_model() -> pa_agent::types::Model {
    serde_json::from_value(json!({
        "id": "faux-1", "name": "Faux", "api": "test", "provider": "faux",
        "baseUrl": "http://localhost", "reasoning": false,
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 200_000, "maxTokens": 4_000
    }))
    .expect("faux loop model")
}

#[tokio::test]
async fn a_closed_fold_is_hidden_from_the_next_request_over_a_real_kernel() {
    let Some(kernel_python) = kernel_python() else {
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let agent_dir = dir.path().join("agent");
    std::fs::create_dir_all(&agent_dir).expect("agent dir");
    std::fs::write(
        agent_dir.join("settings.json"),
        json!({ "contextFolding": { "enabled": true } }).to_string(),
    )
    .expect("settings");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&cwd).expect("project dir");
    let _env = EnvOverride::apply(&[
        (
            "PRIME_AGENT_KERNEL_PYTHON",
            Some(kernel_python.display().to_string()),
        ),
        ("PRIME_AGENT_CODING_AGENT_DIR", None),
        ("PRIME_API_KEY", None),
    ]);

    let model = scripted_model();
    let provider = std::sync::Arc::new(ScriptedProvider::new(model.clone()));
    let cells = [
        (
            "c1",
            fold_cell(
                "fold.begin",
                &json!({ "description": "explore", "goal": "find the answer" }),
            ),
        ),
        ("c2", "print(\"WORK_ONE\")".to_string()),
        ("c3", "print(\"WORK_TWO\")".to_string()),
        (
            "c4",
            fold_cell("fold.end", &json!({ "result": "the answer is 42" })),
        ),
    ];
    for (id, code) in &cells {
        provider.push_tool_call_turn(None, vec![(*id, "ipython", json!({ "code": code }))]);
    }
    provider.push_text_turn("done");

    let engine = create_session(SessionEngineConfig {
        cwd,
        agent_dir,
        model: Some(model),
        stream_fn: Some(provider.stream_fn()),
        // The cells call the bridge directly; the skill package stays out
        // of the kernel install.
        extra_builtin_skill_overrides: vec!["-fold/SKILL.md".to_string()],
        ..SessionEngineConfig::default()
    })
    .await
    .expect("create_session");
    engine
        .prompt("fold the exploration", PromptOptions::default())
        .await
        .expect("prompt");
    engine.session.agent().wait_for_idle().await;

    // One request per scripted turn: after the prompt, after each cell.
    let requests: Vec<Vec<String>> = provider
        .calls()
        .iter()
        .map(|call| {
            call.messages
                .iter()
                .map(|message| serde_json::to_string(message).expect("message json"))
                .collect()
        })
        .collect();
    assert_eq!(requests.len(), 5, "{requests:#?}");
    let carries = |request: &[String], needle: &str| request.iter().any(|m| m.contains(needle));

    // In-fold requests carry each work cell's output with the reminder.
    let reminder = "[fold open: explore]";
    assert!(carries(&requests[2], "WORK_ONE"));
    assert!(requests[2].last().is_some_and(|m| m.contains(reminder)));
    assert!(requests[3].last().is_some_and(|m| m.contains(reminder)));
    assert!(
        !carries(&requests[1], reminder),
        "the begin cell gets no line"
    );

    // After the end cell the work cells are gone, the result stays.
    let after = &requests[4];
    assert!(
        !carries(after, "WORK_ONE") && !carries(after, "WORK_TWO"),
        "{after:#?}"
    );
    assert!(
        carries(after, "Fold closed: explore (4 messages"),
        "{after:#?}"
    );
    assert!(carries(after, "the answer is 42"));
    assert!(!carries(after, reminder));

    // The prefix up to the begin cell's group is byte-identical between the
    // request right after the begin cell and the one after the end cell.
    let prefix = &requests[1];
    assert_eq!(&after[..prefix.len()], prefix.as_slice());

    // The session keeps the full history, without the request-only line,
    // and the end cell's details carry the closed fold's stats.
    let entries = engine.session.entries().await;
    let log = serde_json::to_string(&entries).expect("entries json");
    assert!(log.contains("WORK_ONE") && log.contains("WORK_TWO"));
    assert!(!log.contains(reminder));
    let end_marker = entries
        .iter()
        .find_map(|entry| {
            let value = serde_json::to_value(entry).ok()?;
            let message = value.get("message")?;
            (message.get("toolCallId")? == "c4")
                .then(|| message.get("details")?.get("contextFold").cloned())
                .flatten()
        })
        .expect("the end cell carries its marker");
    assert_eq!(end_marker["kind"], "end", "{end_marker}");
    assert_eq!(end_marker["folded"], true, "{end_marker}");
    assert_eq!(end_marker["messages"], 4, "{end_marker}");
    assert_eq!(end_marker["result"], "the answer is 42", "{end_marker}");
}
