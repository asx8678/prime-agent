#!/usr/bin/env python3
"""Paired before/after analysis for the prime-agent bench harness.

Reads results/<run_id>/<config>/<task>/rep<N>/result.json and reports per-config
aggregates, per-task breakdowns, and paired deltas between the baseline
config and every other config (exact McNemar over solve flips, median token
delta) — the same analysis as pi-fabric bench/analyze.py, over prime-agent's
metrics (recorded cache-aware cost, read-pathology from the session rows).

Synthetic fixtures only (test_analyze.py); never contacts the network.
"""
from __future__ import annotations

import glob
import json
import math
import os
import statistics
import sys


def load_json(path):
    try:
        with open(path) as fh:
            return json.load(fh)
    except (OSError, json.JSONDecodeError):
        return {}


def collect(run_dir):
    cells = []
    for result in glob.glob(os.path.join(run_dir, "*", "*", "rep*", "result.json")):
        cell = os.path.dirname(result)
        rel = os.path.relpath(cell, run_dir)
        config, task, rep = rel.split(os.sep)
        cells.append({"config": config, "task": task, "rep": rep, "path": cell})
    return cells


def exact_mcnemar(a, b):
    """Two-sided exact McNemar p over discordant pairs (a: baseline-only, b: other-only)."""
    n = a + b
    if n == 0:
        return 1.0
    k = min(a, b)
    tail = sum(math.comb(n, i) for i in range(k + 1)) / (2**n)
    return min(1.0, 2 * tail)


def median(rows, key):
    vals = [r.get(key) for r in rows if r.get(key) is not None]
    return statistics.median(vals) if vals else None


def summarize(rows):
    solved = sum(1 for r in rows if r.get("reward_binary") == 1)
    partials = [r["reward_partial"] for r in rows if r.get("reward_partial") is not None]
    reads = sum(r.get("reads", 0) for r in rows)
    whole = sum(r.get("whole_file_reads", 0) for r in rows)
    return {
        "n": len(rows),
        "solves": solved,
        "mean_partial": round(statistics.mean(partials), 4) if partials else None,
        "median_tokens": median(rows, "combined_total_tokens"),
        "median_fresh_input": median(rows, "tokens_fresh_input"),
        "median_cached_input": median(rows, "tokens_cached_input"),
        "median_output": median(rows, "tokens_output"),
        "median_cost": median(rows, "combined_cost_usd"),
        "median_wall_s": median(rows, "agent_wall_s"),
        "median_turns": median(rows, "turns"),
        "median_tool_calls": median(rows, "tool_calls"),
        "reads": reads,
        "whole_file_reads_pct": round(100.0 * whole / reads, 1) if reads else None,
        "results_over_50kb": sum(r.get("results_over_50kb", 0) for r in rows),
    }


def paired_summary(pairs, baseline, other):
    paired = [
        (pv[baseline], pv[other])
        for pv in pairs.values()
        if baseline in pv and other in pv
    ]
    left_only = sum(
        1 for a, b in paired if a.get("reward_binary") == 1 and b.get("reward_binary") != 1
    )
    right_only = sum(
        1 for a, b in paired if a.get("reward_binary") != 1 and b.get("reward_binary") == 1
    )
    tok_delta = [
        b["combined_total_tokens"] - a["combined_total_tokens"]
        for a, b in paired
        if a.get("combined_total_tokens") is not None and b.get("combined_total_tokens") is not None
    ]
    cost_delta = [
        b["combined_cost_usd"] - a["combined_cost_usd"]
        for a, b in paired
        if a.get("combined_cost_usd") is not None and b.get("combined_cost_usd") is not None
    ]
    return {
        "n_pairs": len(paired),
        "solve_flips_baseline_only": left_only,
        "solve_flips_other_only": right_only,
        "mcnemar_p": round(exact_mcnemar(left_only, right_only), 4),
        "median_token_delta": statistics.median(tok_delta) if tok_delta else None,
        "mean_token_delta": round(statistics.mean(tok_delta)) if tok_delta else None,
        "median_cost_delta_usd": (
            round(statistics.median(cost_delta), 6) if cost_delta else None
        ),
    }


def analyze(run_dir):
    cells = collect(run_dir)
    rows = []
    for cell in cells:
        res = load_json(os.path.join(cell["path"], "result.json"))
        rows.append({**cell, **res})
    configs = sorted({r["config"] for r in rows})
    summary = {"run_dir": run_dir, "per_config": {}, "per_task": {}}
    for cfg in configs:
        summary["per_config"][cfg] = summarize([r for r in rows if r["config"] == cfg])
    for task in sorted({r["task"] for r in rows}):
        summary["per_task"][task] = {
            cfg: summarize([r for r in rows if r["task"] == task and r["config"] == cfg])
            for cfg in configs
            if any(r["task"] == task and r["config"] == cfg for r in rows)
        }
    pairs = {}
    for r in rows:
        pairs.setdefault((r["task"], r["rep"]), {})[r["config"]] = r
    if "baseline" in configs:
        for other in (c for c in configs if c != "baseline"):
            summary[f"paired_baseline_vs_{other}"] = paired_summary(pairs, "baseline", other)
    return summary


def main():
    if len(sys.argv) != 2:
        print("usage: analyze.py <run-dir>", file=sys.stderr)
        sys.exit(2)
    summary = analyze(sys.argv[1])
    out_path = os.path.join(sys.argv[1], "analysis-summary.json")
    with open(out_path, "w") as fh:
        json.dump(summary, fh, indent=2)
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
