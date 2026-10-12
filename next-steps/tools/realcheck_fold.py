"""Real-model check, on the zro provider only, of context folding
(next-steps/05b step 7), through the repo's own pa-daemon supervisor.

Everything lives in a throwaway agent dir whose models.json holds only the zro
provider and whose settings turn `contextFolding` on; the user's
~/.prime/agent is read once (to copy that provider) and never written. The
task is synthetic: explore a small generated repo, then fix a planted bug.
Telemetry is off (DO_NOT_TRACK=1).

Two sessions run the same task: one without any mention of folding (does the
model fold unprompted, from the skill inventory alone?) and one told to use
the fold skill. For each it reports the fold markers in the session JSONL,
the prompt size of the request before and after each closed fold (it should
drop), provider errors, and whether the bug got fixed.

Usage (from the repo root, after `cargo build -p pa-daemon`):
    python3 next-steps/tools/realcheck_fold.py target/debug/pa-daemon
It prints a JSON report and copies both session files next to this script
as fold-unprompted-session.jsonl and fold-hinted-session.jsonl.
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
FOLD_KEY = "contextFold"

root = tempfile.mkdtemp(prefix="pafold")  # $TMPDIR keeps the socket paths short
agent = os.path.join(root, "agent")
os.makedirs(agent)
with open(os.path.expanduser("~/.prime/agent/models.json")) as source:
    zro = json.load(source)["providers"]["zro"]
with open(os.path.join(agent, "models.json"), "w") as target:
    json.dump({"providers": {"zro": zro}}, target)
with open(os.path.join(agent, "settings.json"), "w") as target:
    json.dump(
        {
            "defaultProvider": "zro",
            "defaultModel": MODEL,
            "compaction": {"enabled": False},
            "contextFolding": {"enabled": True},
        },
        target,
    )

# A small repo with one planted bug: `mean` divides by one too many.
REPO_FILES = {
    "stats/__init__.py": "",
    "stats/core.py": (
        "def total(values):\n    return sum(values)\n\n\n"
        "def mean(values):\n    if not values:\n        raise ValueError(\"mean of nothing\")\n"
        "    return total(values) / (len(values) + 1)\n"
    ),
    "stats/spread.py": (
        "from .core import mean\n\n\n"
        "def variance(values):\n    centre = mean(values)\n"
        "    return sum((value - centre) ** 2 for value in values) / len(values)\n"
    ),
    "stats/report.py": (
        "from .core import mean, total\nfrom .spread import variance\n\n\n"
        "def summary(values):\n"
        "    return {\"total\": total(values), \"mean\": mean(values), \"variance\": variance(values)}\n"
    ),
    "tests/test_stats.py": (
        "import unittest\n\nfrom stats.report import summary\n\n\n"
        "class SummaryTest(unittest.TestCase):\n"
        "    def test_summary(self):\n"
        "        self.assertEqual(summary([2, 4, 6]), {\"total\": 12, \"mean\": 4.0, \"variance\": 8 / 3})\n\n\n"
        "if __name__ == \"__main__\":\n    unittest.main()\n"
    ),
    "README.md": "A tiny statistics package. Run the tests with `python3 -m unittest discover -s tests`.\n",
}

TASK = (
    "The test in tests/test_stats.py fails. Explore the stats package to find why, "
    "fix the bug in the source (not the test), and run the tests to confirm they pass. "
    "Finish with one sentence naming the file and the fix."
)
HINT = (
    " Use the fold skill: wrap the exploration in fold.begin / fold.end, and the fix "
    "plus test run in a second fold, so only the results stay in your context."
)


def make_repo(path):
    for name, text in REPO_FILES.items():
        target = os.path.join(path, name)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "w") as handle:
            handle.write(text)


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


def messages(session_dir):
    return [row["message"] for row in rows(session_dir) if row["type"] == "message"]


def prompt_size(message):
    """The whole prompt one request sent: uncached plus cached input."""
    usage = message.get("usage") or {}
    return sum(usage.get(key, 0) for key in ("input", "cacheRead", "cacheWrite"))


def analyze(session_dir, work):
    log = messages(session_dir)
    markers = []
    closes = []
    for index, message in enumerate(log):
        if message["role"] != "toolResult":
            continue
        marker = (message.get("details") or {}).get(FOLD_KEY)
        if not marker:
            continue
        markers.append(marker)
        if marker.get("kind") == "end" and marker.get("folded"):
            before = [m for m in log[:index] if m["role"] == "assistant"]
            after = [m for m in log[index:] if m["role"] == "assistant"]
            closes.append(
                {
                    "description": marker.get("description"),
                    "hidden messages": marker.get("messages"),
                    "hidden tokens (estimate)": marker.get("tokens"),
                    "prompt before the close": prompt_size(before[-1]) if before else None,
                    "prompt after the close": prompt_size(after[0]) if after else None,
                }
            )
    fold_calls = [
        call["arguments"].get("code", "")
        for message in log
        if message["role"] == "assistant"
        for call in message.get("content", [])
        if isinstance(call, dict) and call.get("type") == "toolCall" and call.get("name") == "ipython"
    ]
    errors = [
        message.get("errorMessage")
        for message in log
        if message["role"] == "assistant" and message.get("stopReason") == "error"
    ]
    tests = subprocess.run(
        [sys.executable, "-m", "unittest", "discover", "-s", "tests"],
        cwd=work,
        capture_output=True,
        text=True,
        check=False,
    )
    return {
        "cells calling fold.begin": sum("fold.begin" in code for code in fold_calls),
        "cells calling fold.end": sum("fold.end" in code for code in fold_calls),
        "markers": [
            {key: marker.get(key) for key in ("kind", "description", "folded", "messages", "tokens", "overBudget")}
            for marker in markers
        ],
        "closed folds": closes,
        "prompt size per request": [prompt_size(m) for m in log if m["role"] == "assistant"],
        "provider errors": errors,
        "tests pass after the run": tests.returncode == 0,
    }


client = Client(sock_path)


def run(name, message):
    work = os.path.join(root, f"work-{name}")
    make_repo(work)
    session_dir = os.path.join(agent, f"sessions-{name}")
    os.makedirs(session_dir)
    created = client.request(
        {"type": "create", "config": {"cwd": work, "sessionDir": session_dir, "name": name}}
    )
    assert created["success"], created
    session_id = created["data"].get("activeSessionId") or created["data"]["id"]
    done = client.request(
        {
            "type": "prompt_and_wait",
            "activeSessionId": session_id,
            "message": message,
            "streamingBehavior": "followUp",
        }
    )
    assert done["success"], done
    return analyze(session_dir, work)


report = {"provider api (which API family this run covers)": zro.get("api"), "model": MODEL}
try:
    report["unprompted"] = run("fold-unprompted", TASK)
    report["hinted"] = run("fold-hinted", TASK + HINT)
finally:
    print(json.dumps(report, indent=2))
    here = os.path.dirname(os.path.abspath(__file__))
    for name in ("fold-unprompted", "fold-hinted"):
        session_dir = os.path.join(agent, f"sessions-{name}")
        for file_name in os.listdir(session_dir) if os.path.isdir(session_dir) else []:
            if file_name.endswith(".jsonl"):
                shutil.copy(os.path.join(session_dir, file_name), os.path.join(here, f"{name}-session.jsonl"))
    try:
        os.killpg(daemon.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    # Workers and kernels started from the throwaway dir go too.
    subprocess.run(["pkill", "-9", "-f", root], check=False)
    time.sleep(0.5)
    shutil.rmtree(root, ignore_errors=True)
