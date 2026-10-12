//! The deterministic compaction engine through the daemon: the folded state a
//! compaction saves on its durable row survives a worker restart (the next
//! worker reads it back from the session file and folds further), and the
//! durable boundary the worker re-pins is the cut the engine folded up to,
//! on both the `compact` command and the `/compact` prompt path.
#![allow(clippy::large_futures)]
// 64-bit-only targets; the narrowing casts sit at bounded OS boundaries.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
// Fn length is a style gate, not correctness.
#![allow(clippy::too_many_lines)]
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Supervisor {
    child: Child,
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[allow(clippy::zombie_processes)]
fn spawn_supervisor(socket: &Path, agent_dir: &Path) -> Supervisor {
    std::fs::create_dir_all(agent_dir).expect("agent dir");
    let child = Command::new(env!("CARGO_BIN_EXE_pa-daemon"))
        .arg("supervisor")
        .arg("--socket")
        .arg(socket)
        .arg("--agent-dir")
        .arg(agent_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("PRIME_API_KEY")
        .env_remove("PRIME_AGENT_CODING_AGENT_DIR")
        // A supervisor killed at teardown must not leak its session workers.
        .env(
            pa_daemon::worker::WORKER_SUPERVISOR_LOST_EXIT_MS_ENV,
            "15000",
        )
        .spawn()
        .expect("spawn pa-daemon supervisor");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if socket.exists() {
            return Supervisor { child };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("supervisor socket never appeared");
}

/// One client connection: request/response, skipping the session events that
/// stream while a response is outstanding.
struct Client {
    reader: BufReader<std::os::unix::net::UnixStream>,
    writer: std::os::unix::net::UnixStream,
}

impl Client {
    fn connect(socket: &Path) -> Client {
        let stream = std::os::unix::net::UnixStream::connect(socket).expect("connect");
        let writer = stream.try_clone().expect("clone");
        let mut client = Client {
            reader: BufReader::new(stream),
            writer,
        };
        let hello = client.read_line();
        assert_eq!(hello["type"], "daemon_hello");
        client
    }

    fn read_line(&mut self) -> Value {
        let mut line = String::new();
        let deadline = Instant::now() + Duration::from_mins(5);
        self.reader
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("timeout");
        loop {
            match self.reader.read_line(&mut line) {
                Ok(0) => panic!("supervisor closed the connection"),
                Ok(_) if line.trim().is_empty() => line.clear(),
                Ok(_) => return serde_json::from_str(line.trim()).expect("parse line"),
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for supervisor line: {error}"
                    );
                }
            }
        }
    }

    fn send_command(&mut self, id: &str, command: &Value) {
        let envelope = json!({
            "type": "command",
            "id": id,
            "protocol": { "name": "prime-agent.daemon", "version": 7 },
            "command": command,
        });
        let mut line = serde_json::to_string(&envelope).expect("serialize");
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .unwrap_or_else(|error| panic!("write command {id}: {error}"));
    }

    fn request(&mut self, id: &str, command: &Value) -> Value {
        self.send_command(id, command);
        let deadline = Instant::now() + Duration::from_mins(5);
        loop {
            assert!(Instant::now() < deadline, "no response for id {id}");
            let line = self.read_line();
            if line.get("id").and_then(Value::as_str) == Some(id) {
                return line;
            }
        }
    }
}

/// A supervisor, one faux-scripted session over the real agent engine with
/// the deterministic engine and a tiny keep window, and the session dir its
/// durable rows land in.
struct Harness {
    dir: tempfile::TempDir,
    _supervisor: Supervisor,
    client: Client,
    session_id: String,
}

fn setup(name: &str) -> Harness {
    setup_with_responses(name, &json!([{ "text": "scripted reply" }]))
}

/// The harness over a script the test controls (a cell turn first, then
/// whatever later prompts repeat).
fn setup_with_responses(name: &str, responses: &Value) -> Harness {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let agent_dir = dir.path().join("agent");
    let session_dir = agent_dir.join("sessions");
    std::fs::create_dir_all(&session_dir).expect("session dir");
    write_settings(dir.path(), 10);
    let script = dir.path().join("faux.json");
    std::fs::write(
        &script,
        json!({
            "engine": "faux",
            "repeatLastResponse": true,
            "responses": responses.clone(),
        })
        .to_string(),
    )
    .expect("write faux script");
    let socket = dir.path().join(format!("{name}.sock"));
    let supervisor = spawn_supervisor(&socket, &agent_dir);
    let mut client = Client::connect(&socket);
    let created = client.request(
        "c1",
        &json!({
            "type": "create",
            "config": {
                "cwd": dir.path().to_string_lossy(),
                "sessionDir": session_dir.to_string_lossy(),
                "script": script.to_string_lossy(),
                "name": name,
            },
        }),
    );
    assert_eq!(created["success"], true, "create failed: {created}");
    let session_id = created["data"]["activeSessionId"]
        .as_str()
        .or_else(|| created["data"]["id"].as_str())
        .expect("active session id")
        .to_string();
    Harness {
        dir,
        _supervisor: supervisor,
        client,
        session_id,
    }
}

/// Automatic compaction stays off, so only the test's own compactions run.
fn write_settings(dir: &Path, keep_recent_tokens: u64) {
    std::fs::write(
        dir.join("agent").join("settings.json"),
        json!({
            "compaction": {
                "enabled": false,
                "engine": "deterministic",
                "keepRecentTokens": keep_recent_tokens,
            }
        })
        .to_string(),
    )
    .expect("write settings");
}

impl Harness {
    fn session_file(&self) -> PathBuf {
        std::fs::read_dir(self.dir.path().join("agent").join("sessions"))
            .expect("session dir readable")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
            .expect("one session file")
    }

    /// The durable rows, in file order (the test never branches, so file
    /// order is branch order).
    fn rows(&self) -> Vec<Value> {
        std::fs::read_to_string(self.session_file())
            .expect("session file readable")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line.trim()).expect("a JSON row"))
            .collect()
    }

    fn compactions(&self) -> Vec<Value> {
        self.rows()
            .into_iter()
            .filter(|row| row["type"] == "compaction")
            .collect()
    }

    /// The durable user messages before the row a compaction keeps from:
    /// exactly the user messages its saved state folded.
    fn users_before(&self, first_kept_entry_id: &Value) -> u64 {
        self.rows()
            .iter()
            .take_while(|row| &row["id"] != first_kept_entry_id)
            .filter(|row| row["type"] == "message" && row["message"]["role"] == "user")
            .count() as u64
    }

    /// A prompt the way the TUI submits one: `streamingBehavior` resumes the
    /// queued-input suspension a manual `compact` leaves behind.
    fn prompt(&mut self, id: &str, message: &str) {
        let done = self.client.request(
            id,
            &json!({
                "type": "prompt_and_wait",
                "activeSessionId": self.session_id,
                "message": message,
                "streamingBehavior": "followUp",
            }),
        );
        assert_eq!(done["success"], true, "prompt {id} failed: {done}");
    }

    fn compact(&mut self, id: &str) -> Value {
        let compacted = self.client.request(
            id,
            &json!({ "type": "compact", "activeSessionId": self.session_id }),
        );
        assert_eq!(
            compacted["success"], true,
            "compact {id} failed: {compacted}"
        );
        compacted
    }

    fn worker_pid(&mut self, id: &str) -> Option<u64> {
        let state = self.client.request(
            id,
            &json!({ "type": "get_state", "activeSessionId": self.session_id }),
        );
        state["data"]["workerPid"].as_u64()
    }

    /// Kill the session's worker and wait until the supervisor serves the
    /// session from a new one.
    fn restart_worker(&mut self) {
        let killed = self.worker_pid("pid-before").expect("a live worker");
        let _ = Command::new("kill")
            .args(["-9", &killed.to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_mins(3);
        let mut poll = 0;
        loop {
            assert!(Instant::now() < deadline, "no new worker took the session");
            poll += 1;
            if self
                .worker_pid(&format!("pid-poll-{poll}"))
                .is_some_and(|pid| pid != killed)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// A carried note, pinned through the kernel bridge in a cell, renders in
/// every deterministic summary — including the one a restarted worker folds
/// from the saved state, and with the note's own durable row on the branch.
#[test]
fn carried_notes_render_in_every_summary_across_a_restart() {
    let cell = "from rlm import host_request\n\
import json\n\
result = await host_request(\"compact.carry\", {\"add\": [\"the public API must stay stable\"]})\n\
print(json.dumps(result))";
    let mut harness = setup_with_responses(
        "deterministic-carry",
        &json!([
            { "content": [
                { "type": "toolCall", "name": "ipython", "arguments": { "code": cell } },
            ] },
            { "text": "carried the constraint" },
        ]),
    );
    // The cell turn pins the note; the prompts give the fold rows to
    // summarize.
    harness.prompt("p1", "first request alpha");
    harness.prompt("p2", "second request bravo");
    harness.prompt("p3", "third request charlie");
    let carry_rows = harness
        .rows()
        .into_iter()
        .filter(|row| row["type"] == "custom_message" && row["customType"] == "compaction_carry")
        .count();
    assert_eq!(carry_rows, 1, "the pinned note persisted as one row");
    let compacted = harness.compact("cp1");
    let first_summary = compacted["data"]["summary"].as_str().unwrap_or_default();
    assert!(
        first_summary.contains("[Carried notes]\n\n- the public API must stay stable [carry:0]"),
        "the first summary carries the note: {first_summary}"
    );

    harness.restart_worker();
    harness.prompt("p4", "fourth request delta");
    harness.prompt("p5", "fifth request echo");
    // The `/compact` prompt: the turn path's durable re-pin.
    harness.prompt("p6", "/compact");
    let compactions = harness.compactions();
    assert_eq!(compactions.len(), 2, "{compactions:?}");
    let second_summary = compactions[1]["summary"].as_str().unwrap_or_default();
    assert!(
        second_summary.contains("- the public API must stay stable [carry:0]"),
        "the restarted worker's summary carries the note from the saved state: {second_summary}"
    );
}

/// The second compaction runs in a worker that never saw the first: it reads
/// the first row's saved state back from the session file and folds the
/// rows since that row's boundary into it.
#[test]
fn the_folded_state_survives_a_worker_restart() {
    let mut harness = setup("deterministic-restart");
    harness.prompt("p1", "first request alpha");
    harness.prompt("p2", "second request bravo");
    harness.prompt("p3", "third request charlie");
    let compacted = harness.compact("cp1");
    assert!(
        compacted["data"]["summary"]
            .as_str()
            .is_some_and(|summary| summary.starts_with("[Deterministic summary]")),
        "{compacted}"
    );
    let first = harness.compactions().remove(0);
    assert_eq!(
        first["structuralState"]["users"].as_u64(),
        Some(harness.users_before(&first["firstKeptEntryId"])),
        "the saved state folded exactly the rows before the durable boundary: {first}"
    );

    harness.restart_worker();
    harness.prompt("p4", "fourth request delta");
    harness.prompt("p5", "fifth request echo");
    // The `/compact` prompt: the turn path's durable re-pin.
    harness.prompt("p6", "/compact");

    let compactions = harness.compactions();
    assert_eq!(compactions.len(), 2, "{compactions:?}");
    let second = &compactions[1];
    let users = second["structuralState"]["users"].as_u64();
    assert_eq!(
        users,
        Some(harness.users_before(&second["firstKeptEntryId"])),
        "the new worker folded on from the saved state: {second}"
    );
    assert!(users > first["structuralState"]["users"].as_u64());
    let summary = second["summary"].as_str().unwrap_or_default();
    assert!(summary.contains("first request alpha"), "{summary}");
    assert!(!summary.contains("[Earlier summaries]"), "{summary}");
}

/// A keep window edited in settings.json while the session runs: the engine
/// still cuts with the window it was built with, and the durable boundary the
/// worker re-pins must be that same cut, or the saved state and the boundary
/// disagree once a later worker reads them back.
#[test]
fn the_durable_boundary_is_the_cut_the_engine_folded_after_a_settings_edit() {
    let mut harness = setup("deterministic-settings-edit");
    harness.prompt("p1", "first request alpha");
    harness.prompt("p2", "second request bravo");
    harness.prompt("p3", "third request charlie");
    write_settings(harness.dir.path(), 1_000_000);
    harness.compact("cp1");
    harness.prompt("p4", "fourth request delta");
    harness.prompt("p5", "fifth request echo");
    harness.prompt("p6", "/compact");
    let compactions = harness.compactions();
    assert_eq!(compactions.len(), 2, "{compactions:?}");
    for compaction in &compactions {
        assert_eq!(
            compaction["structuralState"]["users"].as_u64(),
            Some(harness.users_before(&compaction["firstKeptEntryId"])),
            "{compaction}"
        );
    }
}
