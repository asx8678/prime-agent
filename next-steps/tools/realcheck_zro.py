"""Real-model check, on the zro provider only, of the first-prompt fix and the
deterministic compaction engine, through the branch's own pa-daemon supervisor.

Everything lives in a throwaway agent dir whose models.json holds only the zro
provider; the user's ~/.prime/agent is read once (to copy that provider) and
never written. Prompts are synthetic. Telemetry is off (DO_NOT_TRACK=1).

Usage (from the repo root, after `cargo build -p pa-daemon`):
    python3 next-steps/tools/realcheck_zro.py target/debug/pa-daemon [--skip-first-prompt]
About 8 short zro calls. It prints a JSON report and copies the main
session file next to this script as main-session.jsonl.
"""
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

DAEMON = sys.argv[1]
MODEL = "glm-5.3-flash"

root = tempfile.mkdtemp(prefix="parc")  # $TMPDIR keeps the socket paths short
agent = os.path.join(root, "agent")
work = os.path.join(root, "work")
os.makedirs(agent)
os.makedirs(work)
with open(os.path.expanduser("~/.prime/agent/models.json")) as source:
    zro = json.load(source)["providers"]["zro"]
with open(os.path.join(agent, "models.json"), "w") as target:
    json.dump({"providers": {"zro": zro}}, target)
with open(os.path.join(agent, "settings.json"), "w") as target:
    json.dump(
        {
            "defaultProvider": "zro",
            "defaultModel": MODEL,
            "compaction": {"enabled": False, "engine": "deterministic", "keepRecentTokens": 10},
        },
        target,
    )

env = dict(os.environ)
env.pop("PRIME_API_KEY", None)
env.pop("PRIME_AGENT_CODING_AGENT_DIR", None)
env["DO_NOT_TRACK"] = "1"
env["PRIME_AGENT_INTERNAL_WORKER_SUPERVISOR_LOST_EXIT_MS"] = "5000"
sock_path = os.path.join(root, "s.sock")
daemon = subprocess.Popen(
    [DAEMON, "supervisor", "--socket", sock_path, "--agent-dir", agent],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=open(os.path.join(root, "daemon.err"), "w"),
    env=env,
    start_new_session=True,
)


class Client:
    def __init__(self, path):
        deadline = time.time() + 10
        while not os.path.exists(path):
            assert time.time() < deadline, "supervisor socket never appeared"
            time.sleep(0.05)
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.connect(path)
        self.file = self.sock.makefile("rwb")
        hello = self.read()
        assert hello["type"] == "daemon_hello", hello
        self.next_id = 0

    def read(self):
        line = self.file.readline()
        assert line, "supervisor closed the connection"
        return json.loads(line)

    def request(self, command):
        self.next_id += 1
        request_id = f"r{self.next_id}"
        envelope = {
            "type": "command",
            "id": request_id,
            "protocol": {"name": "prime-agent.daemon", "version": 7},
            "command": command,
        }
        self.file.write((json.dumps(envelope) + "\n").encode())
        self.file.flush()
        while True:
            line = self.read()
            if line.get("id") == request_id:
                return line


def rows(session_dir):
    [name] = [n for n in os.listdir(session_dir) if n.endswith(".jsonl")]
    with open(os.path.join(session_dir, name)) as source:
        return [json.loads(line) for line in source if line.strip()]


def assistant_inputs(session_dir):
    """The whole prompt each request sent: uncached plus cached input."""
    return [
        sum(row["message"]["usage"].get(key, 0) for key in ("input", "cacheRead", "cacheWrite"))
        for row in rows(session_dir)
        if row["type"] == "message" and row["message"]["role"] == "assistant"
    ]


def assistant_outputs(session_dir):
    return [
        row["message"]["usage"]["output"]
        for row in rows(session_dir)
        if row["type"] == "message" and row["message"]["role"] == "assistant"
    ]


def last_reply(session_dir):
    for row in reversed(rows(session_dir)):
        if row["type"] == "message" and row["message"]["role"] == "assistant":
            content = row["message"]["content"]
            if isinstance(content, str):
                return content
            return " ".join(block.get("text", "") for block in content if block.get("type") == "text")
    return ""


def users_before(session_dir, first_kept):
    count = 0
    for row in rows(session_dir):
        if row["id"] == first_kept:
            return count
        if row["type"] == "message" and row["message"]["role"] == "user":
            count += 1
    return count


client = Client(sock_path)


def create(name):
    session_dir = os.path.join(agent, f"sessions-{name}")
    os.makedirs(session_dir)
    created = client.request(
        {"type": "create", "config": {"cwd": work, "sessionDir": session_dir, "name": name}}
    )
    assert created["success"], created
    return created["data"].get("activeSessionId") or created["data"]["id"], session_dir


def prompt(session_id, message):
    done = client.request(
        {
            "type": "prompt_and_wait",
            "activeSessionId": session_id,
            "message": message,
            "streamingBehavior": "followUp",
        }
    )
    assert done["success"], done


NO_CODE = " Answer in plain text; do not run code or tools."
FILLER = " ".join(f"note-{index}" for index in range(900))
LONG = f"Background notes for later, ignore them for now: {FILLER}. Reply with just: noted." + NO_CODE
SHORT = "Reply with just: ok." + NO_CODE
report = {}
try:
    # 1. First-prompt fix: the long prompt's cost as a later prompt versus as
    #    the first prompt of a session. Sent once, both deltas match.
    if "--skip-first-prompt" not in sys.argv:
        base_id, base_dir = create("rc-base")
        prompt(base_id, SHORT)
        prompt(base_id, LONG)
        inputs, outputs = assistant_inputs(base_dir), assistant_outputs(base_dir)
        report["long prompt tokens as a later prompt"] = inputs[1] - inputs[0] - outputs[0]
        long_id, long_dir = create("rc-long")
        prompt(long_id, LONG)
        report["first-request input, short vs long first prompt"] = [inputs[0], assistant_inputs(long_dir)[0]]
        report["long prompt tokens as a first prompt"] = assistant_inputs(long_dir)[0] - inputs[0]
    main_id, main_dir = create("rc-main")

    # 2. Deterministic compaction on a real conversation.
    prompt(main_id, "Remember the code word AZURE-FALCON-42; I will ask for it later. Reply with just: ok." + NO_CODE)
    prompt(main_id, "List three numbered steps for testing a login form, in under 60 words." + NO_CODE)
    prompt(main_id, "Which of those steps is riskiest? One sentence." + NO_CODE)
    compacted = client.request({"type": "compact", "activeSessionId": main_id})
    assert compacted["success"], compacted
    first = [row for row in rows(main_dir) if row["type"] == "compaction"][0]
    report["compaction 1"] = {
        "summary starts": first["summary"][:40],
        "summary bytes": len(first["summary"]),
        "usage on the row": first.get("usage"),
        "saved users": first["structuralState"]["users"],
        "file users before the boundary": users_before(main_dir, first["firstKeptEntryId"]),
        "code word in summary": "AZURE-FALCON-42" in first["summary"],
    }
    prompt(main_id, "What was the code word I asked you to remember? Reply with just the code word." + NO_CODE)
    report["reply after compaction"] = last_reply(main_dir)[:120]

    # 3. Restart the worker, then compact again on the new worker.
    state = client.request({"type": "get_state", "activeSessionId": main_id})
    killed = state["data"]["workerPid"]
    os.kill(killed, signal.SIGKILL)
    deadline = time.time() + 180
    while True:
        assert time.time() < deadline, "no new worker took the session"
        pid = client.request({"type": "get_state", "activeSessionId": main_id}).get("data", {}).get("workerPid")
        if pid and pid != killed:
            break
        time.sleep(0.2)
    prompt(main_id, "In one sentence, what did we discuss about login testing?" + NO_CODE)
    prompt(main_id, "/compact")
    second = [row for row in rows(main_dir) if row["type"] == "compaction"][1]
    report["compaction 2 (new worker)"] = {
        "saved users": second["structuralState"]["users"],
        "file users before the boundary": users_before(main_dir, second["firstKeptEntryId"]),
        "code word in summary": "AZURE-FALCON-42" in second["summary"],
        "earlier summaries block": "[Earlier summaries]" in second["summary"],
    }
finally:
    print(json.dumps(report, indent=2))
    main_sessions = os.path.join(agent, "sessions-rc-main")
    for name in os.listdir(main_sessions) if os.path.isdir(main_sessions) else []:
        shutil.copy(os.path.join(main_sessions, name), os.path.join(os.path.dirname(os.path.abspath(__file__)), "main-session.jsonl"))
    try:
        os.killpg(daemon.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    # Workers and kernels started from the throwaway dir go too.
    subprocess.run(["pkill", "-9", "-f", root], check=False)
    time.sleep(0.5)
    shutil.rmtree(root, ignore_errors=True)
