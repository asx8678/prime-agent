//! The engine unit battery: the scripted tool loop, the child-depth
//! stamp, the MCP-gating unlock, and the goal/heartbeat handler
//! registration.
use super::*;
use crate::session_engine::tool_bridge::{bridge_tool, ToolDefinitionBridge};
use crate::tools::tool_definition::{ExecutionMode, ToolDefinition, ToolExecutionResult};
use pa_agent::scripted::ScriptedProvider;

fn echo_definition() -> ToolDefinition {
    ToolDefinition {
        name: "echo".to_string(),
        label: "Echo".to_string(),
        description: "Echoes its input".to_string(),
        prompt_snippet: String::new(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        }),
        execution_mode: Some(ExecutionMode::Sequential),
        prepare_arguments: None,
        execute: Arc::new(|_id, params, _signal, _on_update| {
            let text = params
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string();
            Box::pin(async move { Ok(ToolExecutionResult::text(format!("echo: {text}"))) })
        }),
    }
}

#[tokio::test]
async fn engine_runs_tool_loop_and_persists() {
    let model = pa_agent::types::Model {
        id: "m".into(),
        name: "m".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 1_000,
        max_tokens: 100,
        max_tokens_explicit: false,
    };
    let provider = Arc::new(ScriptedProvider::new(model.clone()));
    provider.push_tool_call_turn(
        Some("checking"),
        vec![("call-1", "echo", serde_json::json!({ "text": "hi" }))],
    );
    provider.push_text_turn("all done");

    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let engine = create_session(SessionEngineConfig {
        on_late_sent_agent_message: None,
        cache_warm_arm: None,
        semantic_edges: None,
        cron_store: None,
        queued_steering_probe: None,
        image_model_router: None,
        steering_mode: None,
        follow_up_mode: None,
        cwd: cwd.clone(),
        agent_dir: tmp.path().join("agent"),
        mcp_manager: None,
        model: Some(model),
        thinking_level: None,
        stream_fn: Some(provider.stream_fn()),
        tools: vec![bridge_tool(echo_definition())],
        custom_system_prompt: None,
        prompt_guidelines: vec![],
        generic_mcp_servers: vec![],
        allow_recursion: None,
        session_manager: None,
        extra_host_handlers: None,
        conversation_log_path: None,
        additional_skill_paths: vec![],
        additional_prompt_paths: vec![],
        extra_builtin_skill_overrides: vec![],
        rlm_subagent_host: None,
        rlm_depth: None,
        telemetry: None,
        model_info: None,
        on_background_work_settled: None,
        prewarm_ipython_kernel: None,
        queued_goal_context_purge: None,
        inherited_cost_ceiling: None,
    })
    .await
    .unwrap();

    // The system prompt is the layered assembly: static core layer
    // first, dynamic tail after.
    assert!(engine.system_prompt.starts_with("# prime-agent harness"));
    assert!(engine
        .system_prompt
        .contains("Recursive agent depth: 0 (root)"));

    let outcome = engine
        .prompt("run the echo tool", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(outcome, PromptOutcome::Prompt);
    engine.session.agent().wait_for_idle().await;

    let state = engine.session.agent().state().await;
    assert!(state.messages.iter().any(|message| match message {
        pa_agent::types::AgentMessage::Standard(pa_agent::types::Message::ToolResult(result)) => {
            result.tool_name == "echo"
        }
        _ => false,
    }));
    assert!(state.messages.iter().any(|message| match message {
        pa_agent::types::AgentMessage::Standard(pa_agent::types::Message::Assistant(assistant)) => {
            assistant.content.iter().any(|block| {
                matches!(
                    block,
                    pa_agent::types::AssistantContent::Text(text) if text.text == "all done"
                )
            })
        }
        _ => false,
    }));
    let entries = engine.session.entries().await;
    assert!(entries.iter().any(|entry| matches!(
        entry,
        pa_types::session::FileEntry::Message {
            message: pa_types::session::AgentMessage::User(user),
            ..
        } if user.content.text() == "run the echo tool"
    )));
    let _ = ToolDefinitionBridge::new;
}

/// The warmer wiring end to end (TS `sdk.ts`'s `cacheWarmer.start` +
/// `CacheWarmer`): a session request arms the warmer through the config's
/// slot, the loop's own usage feeds the prompt size, and after the refresh
/// delay the one-token replay hits the provider. The settings' global
/// `cacheWarming: "idle"` keeps warming alive past the run's settle.
#[tokio::test]
async fn the_engine_arms_and_refreshes_the_prompt_cache() {
    let registration =
        pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
            api: Some("engine-warming-test".to_owned()),
            ..Default::default()
        });
    let max_tokens_seen: Arc<std::sync::Mutex<Vec<Option<u64>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_for_factory = Arc::clone(&max_tokens_seen);
    let factory = pa_ai::faux::FauxResponseStep::Factory(Arc::new(move |_, options, _, _| {
        seen_for_factory
            .lock()
            .unwrap()
            .push(options.and_then(|options| options.max_tokens));
        let mut message = pa_ai::faux::faux_assistant_text_message(
            "ok",
            pa_ai::faux::FauxAssistantMessageOptions::default(),
        );
        message.usage.input = 1_000_000;
        Ok(message)
    }));
    registration.set_responses(vec![factory; 4]);
    let mut model = registration.get_model();
    model.prompt_cache = Some(pa_types::ai::ModelPromptCache {
        short: Some(11),
        long: None,
    });
    model.cost = pa_types::ai::ModelCost {
        input: pa_types::JsNumber::from(300.0),
        output: pa_types::JsNumber::from(15.0),
        cache_read: pa_types::JsNumber::from(30.0),
        cache_write: pa_types::JsNumber::from(375.0),
    };

    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("settings.json"),
        r#"{ "cacheWarming": "idle" }"#,
    )
    .unwrap();

    let warm_arm: std::sync::Arc<
        std::sync::Mutex<Option<crate::session_engine::provider_adapter::WarmArm>>,
    > = Arc::new(std::sync::Mutex::new(None));
    let session_manager = crate::session::manager::SessionManager::in_memory(&cwd);
    let stream_fn = {
        let slot = Arc::clone(&warm_arm);
        let arm_reader: crate::session_engine::provider_adapter::WarmArm =
            Arc::new(move |request| {
                if let Some(arm) = slot.lock().unwrap().as_ref() {
                    arm(request);
                }
            });
        crate::session_engine::provider_adapter::warming_stream_fn(
            Arc::new(std::sync::RwLock::new(Some(
                crate::session_engine::provider_adapter::ProviderTarget {
                    api_key: None,
                    model: model.clone(),
                    service_tier: None,
                    headers: None,
                },
            ))),
            session_manager.get_session_id().to_string(),
            arm_reader,
        )
    };
    let engine = create_session(SessionEngineConfig {
        on_late_sent_agent_message: None,
        cache_warm_arm: Some(Arc::clone(&warm_arm)),
        stream_fn: Some(stream_fn),
        cwd: cwd.clone(),
        agent_dir,
        session_manager: Some(session_manager),
        model: Some(
            crate::session_engine::provider_adapter::json_round_trip(&model)
                .expect("model conversion"),
        ),
        telemetry: None,
        prewarm_ipython_kernel: Some(false),
        ..SessionEngineConfig::default()
    })
    .await
    .unwrap();
    let warm_usages: Arc<std::sync::Mutex<Vec<crate::session_engine::cache_warmer::WarmUsage>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let warm_usages_for_sink = Arc::clone(&warm_usages);
    engine
        .session
        .cache_warmer()
        .expect("the engine built a warmer")
        .set_usage_sink(std::sync::Arc::new(move |usage| {
            warm_usages_for_sink.lock().unwrap().push(usage);
        }));

    let outcome = engine.prompt("hi", PromptOptions::default()).await.unwrap();
    assert_eq!(outcome, PromptOutcome::Prompt);
    engine.session.agent().wait_for_idle().await;
    assert!(
        warm_arm.lock().unwrap().is_some(),
        "the session request armed the warmer through the config slot"
    );

    // The TTL is 11 s: the first refresh is due after 1 s, past the
    // streaming settle (idle mode keeps it armed).
    for _ in 0..100 {
        if max_tokens_seen.lock().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(
        *max_tokens_seen.lock().unwrap(),
        vec![None, Some(1)],
        "the real request streams uncapped, the refresh replays with a one-token cap"
    );
    let usages = warm_usages.lock().unwrap();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0].model, model.id);
    registration.unregister();
}

/// A spawned child's prompt stamps its recursion depth: `create_session`
/// at depth N reads "depth: N (not root)", never the root identity.
#[tokio::test]
async fn spawned_child_prompt_stamps_its_depth() {
    let model = pa_agent::types::Model {
        id: "m".into(),
        name: "m".into(),
        api: "test".into(),
        provider: "test".into(),
        base_url: "http://localhost".into(),
        reasoning: false,
        cost: pa_agent::types::UsageCost::default(),
        context_window: 1_000,
        max_tokens: 100,
        max_tokens_explicit: false,
    };
    let provider = Arc::new(ScriptedProvider::new(model.clone()));
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let engine = create_session(SessionEngineConfig {
        on_late_sent_agent_message: None,
        cache_warm_arm: None,
        semantic_edges: None,
        cron_store: None,
        queued_steering_probe: None,
        image_model_router: None,
        steering_mode: None,
        follow_up_mode: None,
        cwd: cwd.clone(),
        agent_dir: tmp.path().join("agent"),
        mcp_manager: None,
        model: Some(model),
        thinking_level: None,
        stream_fn: Some(provider.stream_fn()),
        tools: vec![bridge_tool(echo_definition())],
        custom_system_prompt: None,
        prompt_guidelines: vec![],
        generic_mcp_servers: vec![],
        allow_recursion: None,
        session_manager: None,
        extra_host_handlers: None,
        conversation_log_path: None,
        additional_skill_paths: vec![],
        additional_prompt_paths: vec![],
        extra_builtin_skill_overrides: vec![],
        rlm_subagent_host: None,
        rlm_depth: Some(2),
        telemetry: None,
        model_info: None,
        on_background_work_settled: None,
        prewarm_ipython_kernel: None,
        queued_goal_context_purge: None,
        inherited_cost_ceiling: None,
    })
    .await
    .unwrap();

    assert!(engine
        .system_prompt
        .contains("Recursive agent depth: 2 (not root)"));
    assert!(!engine.system_prompt.contains("depth: 0 (root)"));
}

/// The login chain's prompt-gating end to end: a settings-declared OAuth
/// server stays gated, an endpoint-bound credential unlocks it in the NEXT
/// session, and one bound to another endpoint does not.
#[tokio::test]
async fn oauth_creds_unlock_generic_mcp_gating_in_new_sessions() {
    fn model() -> pa_agent::types::Model {
        pa_agent::types::Model {
            id: "m".into(),
            name: "m".into(),
            api: "test".into(),
            provider: "test".into(),
            base_url: "http://localhost".into(),
            reasoning: false,
            cost: pa_agent::types::UsageCost::default(),
            context_window: 1_000,
            max_tokens: 100,
            max_tokens_explicit: false,
        }
    }

    fn config(
        cwd: &std::path::Path,
        agent_dir: &std::path::Path,
        stream_fn: pa_agent::stream::StreamFn,
    ) -> SessionEngineConfig {
        SessionEngineConfig {
            on_late_sent_agent_message: None,
            semantic_edges: None,
            cron_store: None,
            queued_steering_probe: None,
            image_model_router: None,
            steering_mode: None,
            follow_up_mode: None,
            cwd: cwd.to_path_buf(),
            agent_dir: agent_dir.to_path_buf(),
            mcp_manager: None,
            model: Some(model()),
            thinking_level: None,
            stream_fn: Some(stream_fn),
            tools: vec![],
            custom_system_prompt: None,
            prompt_guidelines: vec![],
            generic_mcp_servers: vec![],
            allow_recursion: None,
            session_manager: None,
            extra_host_handlers: None,
            conversation_log_path: None,
            additional_skill_paths: vec![],
            additional_prompt_paths: vec![],
            extra_builtin_skill_overrides: vec![],
            rlm_subagent_host: None,
            rlm_depth: None,
            telemetry: None,
            cache_warm_arm: None,
            model_info: None,
            on_background_work_settled: None,
            prewarm_ipython_kernel: None,
            queued_goal_context_purge: None,
            inherited_cost_ceiling: None,
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    // The settings declaration the daemon worker's MCP manager also
    // resolves (an OAuth HTTP server, like `mcp add ... --oauth`).
    std::fs::write(
        agent_dir.join("settings.json"),
        serde_json::json!({
            "mcpServers": {
                "fixture-oauth": {
                    "type": "http",
                    "url": "https://fixture.example/mcp",
                    "oauth": true,
                },
            },
        })
        .to_string(),
    )
    .unwrap();
    let provider = Arc::new(ScriptedProvider::new(model()));

    let engine = create_session(config(&cwd, &agent_dir, provider.stream_fn()))
        .await
        .unwrap();
    assert!(!engine.system_prompt.contains("# Generic MCP Connections"));

    // The persisted credential begin_login leaves behind (the TS
    // McpCredentials shape, endpoint-bound).
    let write_credential = |endpoint: &str| {
        std::fs::write(
            agent_dir.join("auth.json"),
            serde_json::json!({
                "mcp:fixture-oauth": {
                    "type": "oauth",
                    "access": "fixture-access",
                    "refresh": "fixture-refresh",
                    "expires": 999_999_999_999_999_i64,
                    "endpoint": endpoint,
                    "tokenEndpoint": "https://fixture.example/token",
                    "clientId": "fixture-client",
                },
            })
            .to_string(),
        )
        .unwrap();
    };

    // A credential bound to another endpoint stays gated: the token
    // must prove where it belongs.
    write_credential("https://other.example/mcp");
    let engine = create_session(config(&cwd, &agent_dir, provider.stream_fn()))
        .await
        .unwrap();
    assert!(!engine.system_prompt.contains("# Generic MCP Connections"));

    write_credential("https://fixture.example/mcp");
    let engine = create_session(config(&cwd, &agent_dir, provider.stream_fn()))
        .await
        .unwrap();
    assert!(engine.system_prompt.contains("# Generic MCP Connections"));
    assert!(engine.system_prompt.contains("`fixture-oauth`"));
}

#[tokio::test]
async fn create_session_registers_goal_and_heartbeat_handlers() {
    let dir = tempfile::TempDir::new().unwrap();
    let registration =
        pa_ai::faux::register_faux_provider(pa_ai::faux::RegisterFauxProviderOptions {
            models: Some(vec![pa_ai::faux::FauxModelDefinition {
                id: "faux-1".to_string(),
                name: Some("Faux".to_string()),
                reasoning: Some(false),
                input: Some(vec![pa_types::ai::ModelInput::Text]),
                cost: None,
                context_window: Some(100_000),
                max_tokens: Some(4_096),
            }]),
            ..Default::default()
        });
    registration.set_responses(vec![pa_ai::faux::FauxResponseStep::Message(
        pa_ai::faux::faux_assistant_text_message(
            "ok",
            pa_ai::faux::FauxAssistantMessageOptions::default(),
        ),
    )]);
    let model = registration.get_model();
    let agent_model = crate::session_engine::provider_adapter::json_round_trip(&model).unwrap();
    let stream_fn = crate::session_engine::provider_adapter::real_stream_fn(None, model.clone());
    let engine = create_session(SessionEngineConfig {
        cron_store: None,
        cwd: dir.path().to_path_buf(),
        agent_dir: dir.path().to_path_buf(),
        model: Some(agent_model),
        stream_fn: Some(stream_fn),
        tools: Vec::new(),
        ..Default::default()
    })
    .await
    .unwrap();
    let names: Vec<String> = engine
        .session
        .agent()
        .state()
        .await
        .tools
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();
    assert!(
        names.iter().any(|name| name == "ipython"),
        "tools: {names:?}"
    );
}

/// A tool whose result carries the context-fold marker it is given, the
/// way the `fold` skill's cells carry theirs through the kernel channel.
fn marking_cell_definition() -> ToolDefinition {
    ToolDefinition {
        name: "cell".to_string(),
        label: "Cell".to_string(),
        description: "Runs a cell".to_string(),
        prompt_snippet: String::new(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "marker": { "type": "object" } }
        }),
        execution_mode: Some(ExecutionMode::Sequential),
        prepare_arguments: None,
        execute: Arc::new(|id, params, _signal, _on_update| {
            let text = format!("ran {id}");
            let details = match params.get("marker") {
                Some(marker) => serde_json::json!({ "status": "ok", "contextFold": marker }),
                None => serde_json::json!({ "status": "ok" }),
            };
            Box::pin(async move {
                Ok(ToolExecutionResult {
                    details: Some(details),
                    ..ToolExecutionResult::text(text)
                })
            })
        }),
    }
}

fn fold_test_model() -> pa_agent::types::Model {
    pa_agent::types::Model {
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
    }
}

/// A session over `provider` with `contextFolding.enabled` set to
/// `folding`, resuming `session_manager`'s entries when given one.
async fn fold_session(
    folding: bool,
    provider: &Arc<ScriptedProvider>,
    session_manager: Option<crate::session::manager::SessionManager>,
) -> (SessionEngine, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(
        agent_dir.join("settings.json"),
        serde_json::json!({ "contextFolding": { "enabled": folding } }).to_string(),
    )
    .unwrap();
    let engine = create_session(SessionEngineConfig {
        cwd,
        agent_dir,
        model: Some(fold_test_model()),
        stream_fn: Some(provider.stream_fn()),
        tools: vec![bridge_tool(marking_cell_definition())],
        session_manager,
        ..Default::default()
    })
    .await
    .unwrap();
    (engine, tmp)
}

fn tool_result_text(content: &[pa_agent::types::ToolResultContent]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            pa_agent::types::ToolResultContent::Text(text) => Some(text.text.as_str()),
            pa_agent::types::ToolResultContent::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Each provider request's messages as compact lines.
fn request_lines(provider: &ScriptedProvider) -> Vec<Vec<String>> {
    provider
        .calls()
        .iter()
        .map(|call| {
            call.messages
                .iter()
                .map(|message| match message {
                    pa_agent::types::Message::User(_) => "user".to_string(),
                    pa_agent::types::Message::Assistant(assistant) => format!(
                        "assistant {}",
                        assistant
                            .tool_calls()
                            .iter()
                            .map(|call| call.id.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                    pa_agent::types::Message::ToolResult(result) => {
                        format!(
                            "result {}: {}",
                            result.tool_call_id,
                            tool_result_text(&result.content)
                        )
                    }
                })
                .collect()
        })
        .collect()
}

fn owned_lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_string()).collect()
}

/// One scripted fold (begin cell, a work cell, end cell, answer) through a
/// real session. Returns the session, each provider request's messages as
/// compact lines, and the session's tool-result texts.
async fn fold_scenario(folding: bool) -> (SessionEngine, Vec<Vec<String>>, Vec<String>) {
    let provider = Arc::new(ScriptedProvider::new(fold_test_model()));
    let begin = serde_json::json!({ "kind": "begin", "description": "explore", "goal": "find it" });
    let end = serde_json::json!({ "kind": "end", "description": "explore", "result": "found it" });
    provider.push_tool_call_turn(
        None,
        vec![("c1", "cell", serde_json::json!({ "marker": begin }))],
    );
    provider.push_tool_call_turn(None, vec![("c2", "cell", serde_json::json!({}))]);
    provider.push_tool_call_turn(
        None,
        vec![("c3", "cell", serde_json::json!({ "marker": end }))],
    );
    provider.push_text_turn("done");
    let (engine, _tmp) = fold_session(folding, &provider, None).await;
    engine
        .prompt("fold it", PromptOptions::default())
        .await
        .unwrap();
    engine.session.agent().wait_for_idle().await;

    let logged = engine
        .session
        .agent()
        .state()
        .await
        .messages
        .iter()
        .filter_map(|message| match message {
            pa_agent::types::AgentMessage::Standard(pa_agent::types::Message::ToolResult(
                result,
            )) => Some(tool_result_text(&result.content)),
            pa_agent::types::AgentMessage::Standard(
                pa_agent::types::Message::User(_) | pa_agent::types::Message::Assistant(_),
            )
            | pa_agent::types::AgentMessage::Custom(_) => None,
        })
        .collect();
    (engine, request_lines(&provider), logged)
}

#[tokio::test]
async fn a_closed_fold_is_left_out_of_later_requests_only_when_folding_is_enabled() {
    const REMINDER: &str = "[fold open: explore] Work only on this fold's goal; when it is done, call await fold.end(result).";
    let with_prefix = |prefix: &[String], rest: &[String]| [prefix, rest].concat();

    let (_engine, requests, logged) = fold_scenario(true).await;
    assert_eq!(requests.len(), 4);
    // The first request carries only the prompt-time rows; every later
    // request starts with them.
    let prefix = requests[0].clone();
    let reminder_result = format!("result c2: ran c2 | {REMINDER}");
    assert_eq!(
        requests[2],
        with_prefix(
            &prefix,
            &owned_lines(&[
                "assistant c1",
                "result c1: ran c1",
                "assistant c2",
                &reminder_result
            ]),
        )
    );
    // After the end cell the work cell is gone; the begin and end cells stay.
    assert_eq!(
        requests[3],
        with_prefix(
            &prefix,
            &owned_lines(&[
                "assistant c1",
                "result c1: ran c1",
                "assistant c3",
                "result c3: ran c3"
            ]),
        )
    );
    // The session keeps the full history, without the request-only line.
    assert_eq!(logged, owned_lines(&["ran c1", "ran c2", "ran c3"]));

    // Folding off: every request carries the whole conversation unchanged.
    let (_engine, requests, logged) = fold_scenario(false).await;
    let prefix = requests[0].clone();
    assert_eq!(
        requests[3],
        with_prefix(
            &prefix,
            &owned_lines(&[
                "assistant c1",
                "result c1: ran c1",
                "assistant c2",
                "result c2: ran c2",
                "assistant c3",
                "result c3: ran c3",
            ]),
        )
    );
    assert_eq!(logged, owned_lines(&["ran c1", "ran c2", "ran c3"]));
}

#[tokio::test]
async fn a_resumed_session_with_a_closed_fold_sends_the_folded_view_only_when_enabled() {
    // The markers live in the saved tool results, so a resumed session
    // folds without any state of its own.
    let (engine, _, _) = fold_scenario(true).await;
    let entries = engine.session.entries().await;
    let mut first_requests = Vec::new();
    for folding in [true, false] {
        let provider = Arc::new(ScriptedProvider::new(fold_test_model()));
        provider.push_text_turn("again");
        let mut manager = crate::session::manager::SessionManager::in_memory(&std::env::temp_dir());
        manager.adopt_entries(entries.clone());
        let (resumed, _tmp) = fold_session(folding, &provider, Some(manager)).await;
        resumed
            .prompt("next", PromptOptions::default())
            .await
            .unwrap();
        resumed.session.agent().wait_for_idle().await;
        first_requests.push(request_lines(&provider).remove(0));
    }
    let work_cell = "result c2: ran c2".to_string();
    assert!(
        !first_requests[0].contains(&work_cell),
        "{first_requests:?}"
    );
    assert!(first_requests[1].contains(&work_cell), "{first_requests:?}");
    // Folding drops exactly the work cell's assistant group.
    let mut unfolded = first_requests[1].clone();
    let at = unfolded.iter().position(|line| *line == work_cell).unwrap();
    unfolded.drain(at - 1..=at);
    assert_eq!(first_requests[0], unfolded);
}
