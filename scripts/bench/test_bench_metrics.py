#!/usr/bin/env python3
"""Tests for bench_metrics (synthetic session fixtures only; no network)."""
import json
import os
import tempfile
import unittest

from scripts.bench.bench_metrics import collect_metrics


def _assistant_row(
    usage=None,
    content_blocks=None,
):
    usage = usage or {}
    return {
        "type": "message",
        "id": "a1",
        "parentId": None,
        "message": {
            "role": "assistant",
            "content": content_blocks or [{"type": "text", "text": "hi"}],
            "usage": {
                "input": usage.get("input", 10),
                "output": usage.get("output", 5),
                "cacheRead": usage.get("cacheRead", 0),
                "cacheWrite": usage.get("cacheWrite", 0),
                "totalTokens": usage.get(
                    "totalTokens",
                    usage.get("input", 10)
                    + usage.get("output", 5)
                    + usage.get("cacheRead", 0)
                    + usage.get("cacheWrite", 0),
                ),
                "cost": {
                    "input": 0,
                    "output": 0,
                    "cacheRead": 0,
                    "cacheWrite": 0,
                    "total": usage.get("cost_total", 0.0),
                },
            },
        },
    }


def _tool_result_row(text, tool_name="ipython"):
    return {
        "type": "message",
        "id": "t1",
        "message": {
            "role": "toolResult",
            "toolCallId": "c1",
            "toolName": tool_name,
            "content": [{"type": "text", "text": text}],
        },
    }


def _tool_call_block(name, arguments, call_id="c1"):
    return {"type": "toolCall", "id": call_id, "name": name, "arguments": arguments}


class CollectMetricsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.session_dir = os.path.join(self.tmp.name, "session")
        os.makedirs(self.session_dir)

    def tearDown(self):
        self.tmp.cleanup()

    def _write(self, rows, filename="s1.jsonl"):
        path = os.path.join(self.session_dir, filename)
        with open(path, "w") as fh:
            for row in rows:
                fh.write(json.dumps(row) + "\n")
        return path

    def test_empty_cell_reports_zeroed_metrics(self):
        self._write([])
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.turns, 0)
        self.assertEqual(metrics.tool_calls, 0)
        self.assertEqual(metrics.reads, 0)
        self.assertIsNone(metrics.as_dict()["whole_file_reads_pct"])

    def test_usage_and_cost_sums_across_rows(self):
        self._write(
            [
                _assistant_row({"input": 100, "output": 20, "cacheRead": 400, "cost_total": 0.5}),
                _assistant_row({"input": 50, "output": 10, "cacheWrite": 30, "cost_total": 0.25}),
            ]
        )
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.turns, 2)
        self.assertEqual(metrics.input_tokens, 150)
        self.assertEqual(metrics.output_tokens, 30)
        self.assertEqual(metrics.cache_read_tokens, 400)
        self.assertEqual(metrics.cache_write_tokens, 30)
        self.assertEqual(metrics.cost_usd_recorded, 0.75)
        self.assertEqual(metrics.as_dict()["tokens_fresh_input"], 180)
        self.assertEqual(metrics.as_dict()["tokens_cached_input"], 400)

    def test_tool_calls_counted_by_name(self):
        self._write(
            [
                _assistant_row(
                    content_blocks=[
                        _tool_call_block("bash", {"command": "ls"}),
                        _tool_call_block("ipython", {"code": "x = 1"}, "c2"),
                    ]
                ),
                _assistant_row(
                    content_blocks=[_tool_call_block("bash", {"command": "pwd"})]
                ),
            ]
        )
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.tool_calls, 3)
        self.assertEqual(metrics.tool_calls_by_name, {"bash": 2, "ipython": 1})

    def test_python_reads_whole_file_share(self):
        self._write(
            [
                _assistant_row(
                    content_blocks=[
                        _tool_call_block(
                            "ipython",
                            {
                                "code": (
                                    "a = p.read_text()\n"
                                    "b = q.read_bytes()\n"
                                    "c = p.read_text()[:100]\n"
                                )
                            },
                        )
                    ]
                )
            ]
        )
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.reads, 3)
        self.assertEqual(metrics.whole_file_reads, 2)
        self.assertEqual(metrics.as_dict()["whole_file_reads_pct"], 66.7)

    def test_bash_cat_counts_whole_file_head_is_bounded(self):
        self._write(
            [
                _assistant_row(
                    content_blocks=[
                        _tool_call_block("bash", {"command": "cat src/main.rs"}),
                        _tool_call_block("bash", {"command": "head -50 log.txt"}),
                    ]
                )
            ]
        )
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.reads, 2)
        self.assertEqual(metrics.whole_file_reads, 1)

    def test_grep_is_not_a_read(self):
        self._write(
            [
                _assistant_row(
                    content_blocks=[
                        _tool_call_block("bash", {"command": "grep -rn TODO src/"})
                    ]
                )
            ]
        )
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.reads, 0)
        self.assertEqual(metrics.whole_file_reads, 0)

    def test_bash_in_ipython_cells_counts_reads(self):
        self._write(
            [
                _assistant_row(
                    content_blocks=[
                        _tool_call_block(
                            "ipython",
                            {
                                "code": (
                                    "await bash('cat src/main.rs')\n"
                                    "r = bash(\"head -50 log.txt\")\n"
                                    "bash('go build ./...')\n"
                                )
                            },
                        )
                    ]
                )
            ]
        )
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.reads, 2)
        self.assertEqual(metrics.whole_file_reads, 1)

    def test_results_over_50kb_counted(self):
        big = "x" * 50_001
        self._write(
            [
                _assistant_row(
                    content_blocks=[_tool_call_block("ipython", {"code": "1"})]
                ),
                _tool_result_row(big),
                _tool_result_row("small"),
            ]
        )
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.results_over_50kb, 1)

    def test_truncated_and_foreign_rows_are_skipped(self):
        path = os.path.join(self.session_dir, "s1.jsonl")
        with open(path, "w") as fh:
            fh.write(json.dumps(_assistant_row()) + "\n")
            fh.write("{not json at all\n")
            fh.write(json.dumps({"type": "compaction", "other": True}) + "\n")
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.turns, 1)

    def test_multiple_session_files_all_read(self):
        self._write([_assistant_row({"input": 10})], "s1.jsonl")
        self._write([_assistant_row({"input": 20})], "s2.jsonl")
        metrics = collect_metrics(self.tmp.name)
        self.assertEqual(metrics.turns, 2)
        self.assertEqual(metrics.input_tokens, 30)


if __name__ == "__main__":
    unittest.main()
