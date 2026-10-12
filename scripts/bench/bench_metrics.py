#!/usr/bin/env python3
"""Metric extraction from prime-agent session JSONL files for the bench harness.

The session file layout is the durable contract this module reads (same layout
as ``~/.prime/agent/sessions``): one JSON object per line. The rows this module
cares about:

- assistant messages (``message.role == "assistant"``) carry ``usage`` with
  ``input``/``output``/``cacheRead``/``cacheWrite``/``totalTokens`` and a
  provider-billed ``cost`` object (``cost.total`` is the recorded cache-aware
  USD total; the ``pa-ai`` ``cache_pricing`` module already priced it), plus
  ``content`` blocks where ``type == "toolCall"`` record every model tool call.
- tool results (``message.role == "toolResult"``) carry the text the model saw
  and the tool that produced it (``toolName``).

Read pathology in prime-agent: the model-facing tool is ``ipython`` (the
engine registers the kernel-backed tool itself; the daemon passes no other
model tools), and file reads happen inside cell code: pathlib
(``.read_text(...)`` / ``.read_bytes(...)``) or the kernel's ``bash()`` helper
with a shell reader (``await bash("cat file")``). Whole-file reads are reads
without a bound: a pathlib ``read_text``/``read_bytes`` call that is not
sliced on the same statement, or a shell ``cat`` (``head``/``tail`` count as
reads but are bounded, so not whole-file). A ``bash`` model tool is parsed
too (config-dependent surfaces may expose one), with the same rules.

Synthetic fixtures only (``test_bench_metrics.py``); this module never
contacts the network.
"""
from __future__ import annotations

import glob
import json
import os
import re
from dataclasses import dataclass, field

# pathlib whole-file readers (a bound would be a slice on the same statement)
_PY_READ_RE = re.compile(r"\.read_text\s*\(|\.read_bytes\s*\(")
_SLICE_RE = re.compile(r"\[\s*:")
# shell whole-file readers; head/tail/nl are bounded and not in this set
_CAT_RE = re.compile(r"(?:^|[;&|]\s*)cat\s+(?:-[\w-]+\s+)*\S")
_BASH_READERS = ("cat", "head", "tail", "less", "more", "nl", "zcat", "bat")
# the kernel's bash() inside ipython cells: await bash("cat file") / r = bash('head -5 f')
_PY_BASH_RE = re.compile(r"\bbash\s*\(\s*(['\"])(.*?)\1", re.S)


@dataclass
class CellMetrics:
    """Aggregated metrics for one benchmark cell (one agent run)."""

    turns: int = 0
    tool_calls: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_write_tokens: int = 0
    total_tokens: int = 0
    cost_usd_recorded: float = 0.0
    reads: int = 0
    whole_file_reads: int = 0
    results_over_50kb: int = 0
    tool_calls_by_name: dict = field(default_factory=dict)

    def as_dict(self):
        return {
            "turns": self.turns,
            "tool_calls": self.tool_calls,
            "tool_calls_by_name": self.tool_calls_by_name,
            "tokens_fresh_input": self.input_tokens + self.cache_write_tokens,
            "tokens_cached_input": self.cache_read_tokens,
            "tokens_output": self.output_tokens,
            "combined_total_tokens": self.total_tokens,
            "combined_cost_usd": round(self.cost_usd_recorded, 6),
            "reads": self.reads,
            "whole_file_reads": self.whole_file_reads,
            "whole_file_reads_pct": (
                round(100.0 * self.whole_file_reads / self.reads, 1)
                if self.reads
                else None
            ),
            "results_over_50kb": self.results_over_50kb,
        }


def session_files(cell_dir):
    """Yield every session JSONL under ``<cell_dir>/session/`` in sorted order."""
    pattern = os.path.join(cell_dir, "session", "*.jsonl")
    yield from sorted(glob.glob(pattern))


def _iter_records(cell_dir):
    for path in session_files(cell_dir):
        with open(path, errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    continue


def _python_read_counts(code):
    """Count (reads, whole-file reads) in one ``ipython`` cell code string.

    An ``ipython`` cell reads files two ways: pathlib (``.read_text()`` /
    ``.read_bytes()``) and the kernel's ``bash()`` helper (``await bash("cat f")``).
    """
    reads = 0
    whole = 0
    for statement in re.split(r"[;\n]", code):
        for _ in _PY_READ_RE.finditer(statement):
            reads += 1
            if not _SLICE_RE.search(statement):
                whole += 1
    for match in _PY_BASH_RE.finditer(code):
        shell_reads, shell_whole = _bash_read_counts(match.group(2))
        reads += shell_reads
        whole += shell_whole
    return reads, whole


def _bash_read_counts(command):
    """Count (reads, whole-file reads) in one ``bash`` tool command string."""
    words = command.split()
    if not words:
        return 0, 0
    head_word = os.path.basename(words[0])
    if head_word not in _BASH_READERS:
        return 0, 0
    # A reader with a pipe destination still read the file; count the read once.
    reads = len(re.findall(r"(?:^|[;&|]\s*)" + re.escape(head_word) + r"\b", command)) or 1
    if head_word == "cat":
        whole = len(_CAT_RE.findall(command)) or (1 if _CAT_RE.search(command) else 0)
    else:
        whole = 0
    return reads, whole


def collect_metrics(cell_dir):
    """Extract :class:`CellMetrics` from one cell's session JSONL files.

    Tolerates truncated or foreign rows the way the session repair layer does:
    undecodable lines and rows without the expected shape are skipped, never
    fatal — a benchmark cell that crashed mid-run still reports the metrics it
    recorded.
    """
    metrics = CellMetrics()
    for record in _iter_records(cell_dir):
        message = record.get("message") or {}
        role = message.get("role")
        if role == "assistant":
            metrics.turns += 1
            usage = message.get("usage") or {}
            metrics.input_tokens += int(usage.get("input") or 0)
            metrics.output_tokens += int(usage.get("output") or 0)
            metrics.cache_read_tokens += int(usage.get("cacheRead") or 0)
            metrics.cache_write_tokens += int(usage.get("cacheWrite") or 0)
            metrics.total_tokens += int(usage.get("totalTokens") or 0)
            cost = (usage.get("cost") or {}).get("total")
            if cost is not None:
                metrics.cost_usd_recorded += float(cost)
            content = message.get("content") or []
            if not isinstance(content, list):
                continue
            for block in content:
                if not isinstance(block, dict):
                    continue
                if block.get("type") != "toolCall":
                    continue
                name = block.get("name") or "unknown"
                metrics.tool_calls += 1
                metrics.tool_calls_by_name[name] = (
                    metrics.tool_calls_by_name.get(name, 0) + 1
                )
                args = block.get("arguments") or {}
                if name == "ipython":
                    reads, whole = _python_read_counts(str(args.get("code") or ""))
                elif name == "bash":
                    reads, whole = _bash_read_counts(str(args.get("command") or ""))
                else:
                    continue
                metrics.reads += reads
                metrics.whole_file_reads += whole
        elif role == "toolResult":
            content = message.get("content") or []
            if not isinstance(content, list):
                continue
            text_len = sum(
                len(p.get("text", ""))
                for p in content
                if isinstance(p, dict)
            )
            if text_len > 50_000:
                metrics.results_over_50kb += 1
    return metrics
