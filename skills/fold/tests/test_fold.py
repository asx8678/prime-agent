"""The fold skill against a fake host bridge: the cell guard, argument
checks, and that it emits exactly the marker the host answers.

Run from the skill directory; ``src`` is inserted on the path here and a
fake ``rlm`` module stands in for the kernel runtime.
"""

import asyncio
import contextlib
import io
import sys
import types
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))


class FakeRepl:
    """The kernel's cell context: by default the calling task is the cell body; a test can
    pin another task as the owner to stand in for a detached task's stale context."""

    def __init__(self):
        self.finished = None
        self.owner = None

    def current_cell_completion_context(self):
        if self.finished is None:
            return None
        return (self.finished, self.owner or asyncio.current_task())


FAKE_REPL = FakeRepl()
HOST_CALLS = []
HOST_REPLIES = []
EMITTED = []


async def fake_host_request(request_type, payload=None):
    HOST_CALLS.append((request_type, payload))
    return HOST_REPLIES.pop(0)


fake_rlm = types.ModuleType("rlm")
fake_rlm.host_request = fake_host_request
fake_rlm.emit = EMITTED.append
fake_rlm.repl = FAKE_REPL
sys.modules["rlm"] = fake_rlm

import fold  # noqa: E402

BEGIN_MARKER = {"kind": "begin", "description": "explore", "goal": "find it"}


class FoldTests(unittest.TestCase):
    def setUp(self):
        HOST_CALLS.clear()
        HOST_REPLIES.clear()
        EMITTED.clear()
        self.new_cell()

    def new_cell(self):
        FAKE_REPL.finished = asyncio.Event()
        FAKE_REPL.owner = None

    def run_call(self, call, *replies):
        HOST_REPLIES.extend(replies)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            asyncio.run(call)
        return out.getvalue()

    def test_begin_prints_and_emits_only_after_the_host_agrees(self):
        printed = self.run_call(
            fold.begin("explore", "find it"),
            {"text": "Fold opened: explore.", "marker": BEGIN_MARKER},
        )
        self.assertEqual(HOST_CALLS, [("fold.begin", {"description": "explore", "goal": "find it"})])
        self.assertEqual(EMITTED, [{fold.CONTEXT_FOLD_DISPLAY_MIME: BEGIN_MARKER}])
        self.assertEqual(printed, "Fold opened: explore.\n")

        self.new_cell()
        printed = self.run_call(
            fold.begin("again", "more"),
            {"text": "Fold not opened: a fold is already open: explore."},
        )
        self.assertEqual(len(EMITTED), 1)
        self.assertEqual(printed, "Fold not opened: a fold is already open: explore.\n")

    def test_a_second_fold_call_in_the_same_cell_is_refused(self):
        self.run_call(fold.begin("explore", "find it"), {"text": "ok", "marker": BEGIN_MARKER})
        with self.assertRaisesRegex(RuntimeError, "already called"):
            self.run_call(fold.end("done"))
        self.assertEqual([call[0] for call in HOST_CALLS], ["fold.begin"])

    def test_calls_outside_a_cell_are_refused(self):
        FAKE_REPL.finished = None
        with self.assertRaisesRegex(RuntimeError, "only work in a cell"):
            self.run_call(fold.begin("explore", "find it"))
        with self.assertRaisesRegex(RuntimeError, "only work in a cell"):
            self.run_call(fold.end("done"))
        self.assertEqual(HOST_CALLS, [])

    def test_a_detached_task_with_a_stale_cell_context_is_refused(self):
        async def cell():
            FAKE_REPL.owner = asyncio.current_task()
            detached = asyncio.create_task(fold.begin("explore", "find it"))
            await detached

        with self.assertRaisesRegex(RuntimeError, "only work in a cell"):
            self.run_call(cell())
        self.assertEqual(HOST_CALLS, [])

    def test_goal_is_required(self):
        with self.assertRaises(TypeError):
            self.run_call(fold.begin("explore"))
        with self.assertRaisesRegex(TypeError, "goal must be str"):
            self.run_call(fold.begin("explore", None))
        self.assertEqual(HOST_CALLS, [])

    def test_end_prints_the_template_and_emits_the_host_stats(self):
        marker = {
            "kind": "end", "description": "explore", "result": "found it", "folded": True,
            "messages": 4, "tokens": 900, "overBudget": False,
        }
        text = "Fold closed: explore (4 messages, ~900 tokens hidden from later turns).\nResult:\nfound it"
        printed = self.run_call(fold.end("found it"), {"text": text, "marker": marker})
        self.assertEqual(HOST_CALLS, [("fold.end", {"result": "found it"})])
        self.assertEqual(EMITTED, [{fold.CONTEXT_FOLD_DISPLAY_MIME: marker}])
        self.assertEqual(printed, text + "\n")

    def test_a_voided_end_emits_a_marker_and_prints_the_reason(self):
        marker = {"kind": "end", "description": "explore", "result": "x", "folded": False}
        text = "Fold not closed: new input arrived during the fold, so nothing was hidden."
        printed = self.run_call(fold.end("x"), {"text": text, "marker": marker})
        self.assertEqual(EMITTED, [{fold.CONTEXT_FOLD_DISPLAY_MIME: marker}])
        self.assertEqual(printed, text + "\n")

    def test_a_refused_end_emits_nothing(self):
        text = "Fold not closed: no fold is open; compaction may have summarized it."
        printed = self.run_call(fold.end("x"), {"text": text})
        self.assertEqual(EMITTED, [])
        self.assertEqual(printed, text + "\n")


if __name__ == "__main__":
    unittest.main()
