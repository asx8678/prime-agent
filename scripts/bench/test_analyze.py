#!/usr/bin/env python3
"""Tests for analyze.py (synthetic result fixtures only; no network)."""
import json
import os
import tempfile
import unittest

from scripts.bench.analyze import analyze, exact_mcnemar


def _write_cell(run_dir, config, task, rep, result):
    cell = os.path.join(run_dir, config, task, f"rep{rep}")
    os.makedirs(cell)
    with open(os.path.join(cell, "result.json"), "w") as fh:
        json.dump(result, fh)


BASE_SOLVED = {
    "reward_binary": 1,
    "reward_partial": 1.0,
    "combined_total_tokens": 1000,
    "combined_cost_usd": 0.01,
    "agent_wall_s": 10.0,
    "turns": 2,
    "tool_calls": 4,
    "reads": 10,
    "whole_file_reads": 5,
    "results_over_50kb": 0,
}


class AnalyzeTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.run_dir = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def test_empty_run_reports_no_configs(self):
        summary = analyze(self.run_dir)
        self.assertEqual(summary["per_config"], {})
        self.assertEqual(summary["per_task"], {})

    def test_per_config_aggregates(self):
        _write_cell(self.run_dir, "baseline", "t1", 0, {**BASE_SOLVED})
        _write_cell(
            self.run_dir,
            "baseline",
            "t1",
            1,
            {**BASE_SOLVED, "reward_binary": 0, "reward_partial": 0.5, "combined_total_tokens": 3000},
        )
        summary = analyze(self.run_dir)
        cfg = summary["per_config"]["baseline"]
        self.assertEqual(cfg["n"], 2)
        self.assertEqual(cfg["solves"], 1)
        self.assertEqual(cfg["mean_partial"], 0.75)
        self.assertEqual(cfg["median_tokens"], 2000)
        self.assertEqual(cfg["whole_file_reads_pct"], 50.0)

    def test_paired_mcnemar_between_baseline_and_other(self):
        # t1: baseline solves, after fails. t2: both solve. t3: after solves, baseline fails.
        _write_cell(self.run_dir, "baseline", "t1", 0, {**BASE_SOLVED})
        _write_cell(self.run_dir, "after", "t1", 0, {**BASE_SOLVED, "reward_binary": 0, "reward_partial": 0.0})
        _write_cell(self.run_dir, "baseline", "t2", 0, {**BASE_SOLVED})
        _write_cell(self.run_dir, "after", "t2", 0, {**BASE_SOLVED})
        _write_cell(self.run_dir, "baseline", "t3", 0, {**BASE_SOLVED, "reward_binary": 0, "reward_partial": 0.0})
        _write_cell(self.run_dir, "after", "t3", 0, {**BASE_SOLVED})
        summary = analyze(self.run_dir)
        paired = summary["paired_baseline_vs_after"]
        self.assertEqual(paired["n_pairs"], 3)
        self.assertEqual(paired["solve_flips_baseline_only"], 1)
        self.assertEqual(paired["solve_flips_other_only"], 1)
        self.assertEqual(paired["mcnemar_p"], 1.0)  # 1 discordant pair each way
        self.assertEqual(paired["median_token_delta"], 0)

    def test_mcnemar_extremes(self):
        # all discordant one way: p = 2 * (1/2^n)
        self.assertAlmostEqual(exact_mcnemar(4, 0), 2 * (1 / 16))
        self.assertEqual(exact_mcnemar(0, 0), 1.0)

    def test_cost_delta_paired(self):
        _write_cell(self.run_dir, "baseline", "t1", 0, {**BASE_SOLVED, "combined_cost_usd": 0.10})
        _write_cell(self.run_dir, "after", "t1", 0, {**BASE_SOLVED, "combined_cost_usd": 0.04})
        summary = analyze(self.run_dir)
        self.assertEqual(summary["paired_baseline_vs_after"]["median_cost_delta_usd"], -0.06)

    def test_per_task_breakdown_lists_both_configs(self):
        _write_cell(self.run_dir, "baseline", "t1", 0, {**BASE_SOLVED})
        _write_cell(self.run_dir, "after", "t1", 0, {**BASE_SOLVED})
        summary = analyze(self.run_dir)
        self.assertIn("baseline", summary["per_task"]["t1"])
        self.assertIn("after", summary["per_task"]["t1"])


if __name__ == "__main__":
    unittest.main()
