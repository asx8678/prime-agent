use pa_types::session::FileEntry;
use proptest::prelude::*;

use super::events::{normalize, CellOutcome, CellStatus, Event, NoticeKind, SentMessage};
use super::render::{render, DEFAULT_MAX_BYTES};
use super::state::{State, STATE_VERSION};
use crate::autonomous::AUTONOMOUS_CONTINUATION_LABEL;

const SESSION: &str = include_str!("../../../tests/fixtures/structural_compaction/session.jsonl");
const SESSION_SUMMARY: &str =
    include_str!("../../../tests/fixtures/structural_compaction/session.summary.txt");

fn entries(jsonl: &str) -> Vec<FileEntry> {
    jsonl
        .lines()
        .map(|line| serde_json::from_str(line).expect("a fixture line parses"))
        .collect()
}

fn text(text: &str) -> String {
    text.to_string()
}

#[test]
fn every_fixture_line_is_a_known_entry() {
    let unknown: Vec<usize> = entries(SESSION)
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, FileEntry::Unknown { .. }))
        .map(|(index, _)| index + 1)
        .collect();
    assert_eq!(
        unknown,
        Vec::<usize>::new(),
        "fixture lines that fell back to Unknown"
    );
}

#[test]
fn the_session_log_normalizes_to_typed_events() {
    let ok = |bash: Option<&str>| CellOutcome {
        bash_first: bash.map(text),
        bash_count: u64::from(bash.is_some()),
        ..CellOutcome::default()
    };
    assert_eq!(
        normalize(&entries(SESSION)),
        vec![
            Event::User {
                text: text("Fix the failing parser test in crates/foo and keep the public API unchanged."),
            },
            Event::AssistantText {
                text: text("I'll run the parser tests first."),
            },
            Event::CellStart {
                call_id: text("call_1"),
                preview: text("r = await bash(\"cargo test -p foo parser\")"),
            },
            Event::CellEnd {
                call_id: text("call_1"),
                outcome: CellOutcome {
                    status: CellStatus::Error,
                    error: Some(text("RuntimeError: tests failed: parser::nested")),
                    ..ok(Some("cargo test -p foo parser"))
                },
            },
            Event::Thinking,
            Event::CellStart {
                call_id: text("call_2"),
                preview: text(
                    "await edit(path=\"crates/foo/src/parser.rs\", old_str=\"depth - 1\", new_str=\"depth.saturating_sub(1)\")"
                ),
            },
            Event::CellEnd {
                call_id: text("call_2"),
                outcome: CellOutcome {
                    edited: vec![text("/repo/crates/foo/src/parser.rs")],
                    ..ok(None)
                },
            },
            Event::CellStart {
                call_id: text("call_3"),
                preview: text("r = await bash(\"cargo test -p foo parser\")"),
            },
            Event::CellEnd {
                call_id: text("call_3"),
                outcome: ok(Some("cargo test -p foo parser")),
            },
            Event::AssistantText {
                text: text("The parser test passes now: `depth` no longer underflows on nested input."),
            },
            Event::User {
                text: text("Good. Ask a subagent to review the change,\nand start the release build in the background."),
            },
            Event::AssistantText {
                text: text("Spawning a reviewer and starting the build."),
            },
            Event::CellStart {
                call_id: text("call_4"),
                preview: text("build = bash(\"cargo build --release\")"),
            },
            Event::CellEnd {
                call_id: text("call_4"),
                outcome: ok(Some("cargo build --release")),
            },
            Event::MessageIn {
                from: text("reviewer"),
                relationship: Some(text("child")),
                text: text("Looks good. One nit: the error path still clones the token."),
            },
            Event::BashDone {
                command: text("cargo build --release"),
                exit_code: Some(101),
            },
            Event::Notice {
                kind: NoticeKind::Heartbeat,
                text: text("Check whether the release build is green."),
            },
            Event::CellStart {
                call_id: text("call_5"),
                preview: text(
                    "await agent_message.send(\"Thanks, fixing the clone.\", receiver_role=\"child\", receiver_name=\"reviewer\")"
                ),
            },
            Event::CellEnd {
                call_id: text("call_5"),
                outcome: CellOutcome {
                    sent: vec![SentMessage {
                        to: text("reviewer"),
                        queued: false,
                    }],
                    ..ok(None)
                },
            },
            Event::ChildEnded {
                name: text("linter"),
                kind: text("failed"),
                detail: Some(text("model quota exceeded\nretry later")),
            },
            Event::BranchSummary {
                text: text("Tried a regex-based parser first; dropped it because it could not handle nested brackets."),
            },
            Event::User {
                text: text("What is left before we can ship? The note </summary> must stay quoted."),
            },
            Event::AssistantText {
                text: text("Remaining: fix the release build (exit 101) and the token clone nit."),
            },
        ]
    );
}

#[test]
fn the_session_summary_matches_its_golden_file() {
    let mut state = State::new();
    state.fold(&normalize(&entries(SESSION)));
    assert_eq!(
        render(
            &state,
            Some("Keep the reviewer's nit in view."),
            DEFAULT_MAX_BYTES
        ),
        SESSION_SUMMARY
    );
}

#[test]
fn switching_engines_keeps_a_labeled_excerpt_of_the_earlier_summary() {
    let state = State::from_summary("## Goal\nShip the parser fix.");
    let summary = render(&state, None, DEFAULT_MAX_BYTES);
    assert!(
        summary.contains(
            "Earlier compaction summary, model-written [u0]:\n## Goal\nShip the parser fix."
        ),
        "got: {summary}"
    );
}

#[test]
fn a_state_saved_by_another_version_is_not_folded_further() {
    let mut saved = State::new().to_value();
    saved["version"] = serde_json::json!(STATE_VERSION + 1);
    assert_eq!(State::from_value(&saved), None);
    assert_eq!(
        State::from_value(&State::new().to_value()),
        Some(State::new())
    );
}

#[test]
fn a_cell_is_labeled_by_its_first_top_level_statement() {
    let code = "import os\n# helper\ndef sh(cmd):\n    return bash(cmd)\n\n@cache\nasync def run():\n    pass\nresult = await sh('make')\n";
    let line = serde_json::json!({
        "type": "message", "id": "e1", "parentId": null, "timestamp": "2026-10-01T09:00:00.000Z",
        "message": {
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": "call_1", "name": "ipython", "arguments": { "code": code } }],
            "api": "anthropic-messages", "provider": "anthropic", "model": "claude-opus-5-5",
            "stopReason": "toolUse", "timestamp": 0,
            "usage": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 } },
        },
    });
    assert_eq!(
        normalize(&entries(&line.to_string())),
        vec![Event::CellStart {
            call_id: text("call_1"),
            preview: text("result = await sh('make')"),
        }]
    );
}

#[test]
fn autonomous_continuations_do_not_displace_user_directions() {
    let user = |id: &str, text: &str| {
        serde_json::json!({
            "type": "message", "id": id, "parentId": null, "timestamp": "2026-10-01T09:00:00.000Z",
            "message": { "role": "user", "content": text, "timestamp": 0 },
        })
        .to_string()
    };
    let continuation = format!("{AUTONOMOUS_CONTINUATION_LABEL}]\n\nNo human input is available.");
    let mut lines = vec![user("e1", "Ship the parser fix.")];
    lines.extend((2..7).map(|id| user(&format!("e{id}"), &continuation)));
    let mut state = State::new();
    state.fold(&normalize(&entries(&lines.join("\n"))));
    let summary = render(&state, None, DEFAULT_MAX_BYTES);
    assert!(
        summary.contains("User instruction [u1]:\nShip the parser fix."),
        "got: {summary}"
    );
    assert!(!summary.contains("User instruction [u2]"), "got: {summary}");
    assert!(
        summary.contains("- [autonomous-continuation] | cells:0 bash:0 edits:0 [u1]"),
        "got: {summary}"
    );
}

#[test]
fn starting_a_command_does_not_resolve_its_exit_failure() {
    let build = |exit_code| Event::BashDone {
        command: text("cargo build --release"),
        exit_code: Some(exit_code),
    };
    let mut state = State::new();
    state.fold(&[
        build(101),
        Event::CellStart {
            call_id: text("call_1"),
            preview: text("bash: cargo build --release"),
        },
        Event::CellEnd {
            call_id: text("call_1"),
            outcome: CellOutcome {
                bash_first: Some(text("cargo build --release")),
                bash_count: 1,
                ..CellOutcome::default()
            },
        },
    ]);
    let started = render(&state, None, DEFAULT_MAX_BYTES);
    assert!(
        started.contains("- bash: cargo build --release: exit 101 [u0]\n"),
        "got: {started}"
    );
    state.fold(&[build(0)]);
    let finished = render(&state, None, DEFAULT_MAX_BYTES);
    assert!(
        finished.contains("- bash: cargo build --release: exit 101 [resolved] [u0]"),
        "got: {finished}"
    );
}

#[test]
fn a_long_dialogue_keeps_the_user_words() {
    let long = |letter: char| letter.to_string().repeat(5_000);
    let mut state = State::new();
    state.fold(&[
        Event::User { text: long('a') },
        Event::AssistantText { text: long('b') },
        Event::User { text: long('c') },
        Event::AssistantText { text: long('d') },
        Event::User { text: long('e') },
        Event::AssistantText { text: long('f') },
    ]);
    let summary = render(&state, None, DEFAULT_MAX_BYTES);
    let dialogue = summary
        .split("\n\n[")
        .find(|block| block.starts_with("Recent user directions"))
        .expect("a dialogue block");
    let shown = |letter: char| dialogue.chars().filter(|c| *c == letter).count();
    assert!(dialogue.len() <= 12_288, "{} bytes", dialogue.len());
    assert!(shown('a') > 0 && shown('f') > 0, "got: {dialogue}");
    assert!(
        shown('c') >= shown('b'),
        "user {} < reply {}",
        shown('c'),
        shown('b')
    );
    assert!(
        shown('e') >= shown('d'),
        "user {} < reply {}",
        shown('e'),
        shown('d')
    );
}

fn words() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-z0-9 .:/\u{e9}\u{754c}\n-]{0,60}",
        1 => "[a-z ]{200,5000}",
        1 => Just("</summary> tail".to_string()),
    ]
}

fn call_id() -> impl Strategy<Value = String> {
    (0u8..6).prop_map(|id| format!("call_{id}"))
}

fn outcome() -> impl Strategy<Value = CellOutcome> {
    (
        prop::sample::select(vec![CellStatus::Ok, CellStatus::Error, CellStatus::Aborted]),
        prop::option::of(words()),
        prop::option::of(prop::sample::select(vec!["cargo test", "make", "ls"])),
        prop::collection::vec("/repo/[a-c]\\.rs", 0..3),
        prop::collection::vec((words(), any::<bool>()), 0..2),
    )
        .prop_map(|(status, error, bash, edited, sent)| CellOutcome {
            status,
            error,
            bash_first: bash.map(str::to_string),
            bash_count: u64::from(bash.is_some()),
            edited,
            sent: sent
                .into_iter()
                .map(|(to, queued)| SentMessage { to, queued })
                .collect(),
        })
}

fn event() -> impl Strategy<Value = Event> {
    prop_oneof![
        words().prop_map(|text| Event::User { text }),
        words().prop_map(|text| Event::AssistantText { text }),
        Just(Event::Thinking),
        (call_id(), words()).prop_map(|(call_id, preview)| Event::CellStart { call_id, preview }),
        (call_id(), outcome()).prop_map(|(call_id, outcome)| Event::CellEnd { call_id, outcome }),
        (call_id(), words(), words()).prop_map(|(call_id, name, subject)| Event::ToolStart {
            call_id,
            name,
            subject,
        }),
        (call_id(), prop::option::of(words()))
            .prop_map(|(call_id, error)| Event::ToolEnd { call_id, error }),
        (
            prop::sample::select(vec!["cargo test", "make"]),
            prop::option::of(-1i64..3)
        )
            .prop_map(|(command, exit_code)| Event::BashDone {
                command: command.to_string(),
                exit_code,
            }),
        (words(), prop::option::of(words()), words()).prop_map(|(from, relationship, text)| {
            Event::MessageIn {
                from,
                relationship,
                text,
            }
        }),
        (
            words(),
            prop::sample::select(vec!["failed", "cancelled"]),
            prop::option::of(words())
        )
            .prop_map(|(name, kind, detail)| Event::ChildEnded {
                name,
                kind: kind.to_string(),
                detail,
            }),
        (
            prop::sample::select(vec![
                NoticeKind::Heartbeat,
                NoticeKind::Goal,
                NoticeKind::Continuation,
                NoticeKind::Refinement,
                NoticeKind::Autonomous,
            ]),
            words()
        )
            .prop_map(|(kind, text)| Event::Notice { kind, text }),
        (words(), words()).prop_map(|(custom_type, text)| Event::Custom { custom_type, text }),
        words().prop_map(|text| Event::BranchSummary { text }),
    ]
}

proptest! {
    #[test]
    fn folding_a_saved_state_further_equals_folding_everything(
        events in prop::collection::vec(event(), 0..120),
        split in 0usize..120,
    ) {
        let split = split.min(events.len());
        let mut whole = State::new();
        whole.fold(&events);
        let mut first = State::new();
        first.fold(&events[..split]);
        let mut resumed = State::from_value(&first.to_value()).expect("a saved state loads");
        resumed.fold(&events[split..]);
        prop_assert_eq!(resumed, whole);
    }

    #[test]
    fn the_summary_stays_within_its_byte_budget(
        events in prop::collection::vec(event(), 0..200),
        budget in prop_oneof![0usize..64, 2_048usize..40_000],
        request in prop::option::of(words()),
    ) {
        let mut state = State::new();
        state.fold(&events);
        let summary = render(&state, request.as_deref(), budget);
        prop_assert!(summary.len() <= budget, "{} bytes over a {budget}-byte budget", summary.len());
        prop_assert!(!summary.contains("</summary>"));
    }
}
