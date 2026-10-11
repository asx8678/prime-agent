"""The row order a fresh session's file starts with, for one pa-daemon binary,
on the faux provider (no model calls): create, one prompt, print the rows.

Usage: python3 next-steps/tools/row_order.py <path to a pa-daemon binary>
Run it against two builds to compare their file order."""
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
root = tempfile.mkdtemp(prefix="paro")
agent = os.path.join(root, "agent")
sessions = os.path.join(agent, "sessions")
os.makedirs(sessions)
script = os.path.join(root, "faux.json")
with open(script, "w") as target:
    json.dump({"engine": "faux", "responses": [{"text": "scripted reply"}]}, target)
env = dict(os.environ)
env.pop("PRIME_API_KEY", None)
env.pop("PRIME_AGENT_CODING_AGENT_DIR", None)
env["DO_NOT_TRACK"] = "1"
env["PRIME_AGENT_INTERNAL_WORKER_SUPERVISOR_LOST_EXIT_MS"] = "3000"
sock_path = os.path.join(root, "s.sock")
daemon = subprocess.Popen(
    [DAEMON, "supervisor", "--socket", sock_path, "--agent-dir", agent],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
    env=env,
    start_new_session=True,
)
try:
    deadline = time.time() + 10
    while not os.path.exists(sock_path):
        assert time.time() < deadline
        time.sleep(0.05)
    sock = socket.socket(socket.AF_UNIX)
    sock.connect(sock_path)
    stream = sock.makefile("rwb")
    stream.readline()

    def request(request_id, command):
        envelope = {"type": "command", "id": request_id, "protocol": {"name": "prime-agent.daemon", "version": 7}, "command": command}
        stream.write((json.dumps(envelope) + "\n").encode())
        stream.flush()
        while True:
            line = json.loads(stream.readline())
            if line.get("id") == request_id:
                return line

    created = request("c1", {"type": "create", "config": {"cwd": root, "sessionDir": sessions, "script": script, "name": "order"}})
    session_id = created["data"].get("activeSessionId") or created["data"]["id"]
    done = request("p1", {"type": "prompt_and_wait", "activeSessionId": session_id, "message": "first prompt"})
    assert done["success"], done
    [name] = [n for n in os.listdir(sessions) if n.endswith(".jsonl")]
    with open(os.path.join(sessions, name)) as source:
        for line in source:
            row = json.loads(line)
            detail = row.get("message", {}).get("role", "") if row["type"] == "message" else row.get("customType", "")
            print(row["type"], detail)
finally:
    try:
        os.killpg(daemon.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    subprocess.run(["pkill", "-9", "-f", root], check=False)
    time.sleep(0.5)
    shutil.rmtree(root, ignore_errors=True)
