# Pass 11: how upstream tests what we want to borrow — and what it costs

Eleventh pass, 2026-10-11 — the verification machinery behind the borrows (durable conformance suites, the eval harness mechanics, bench task anatomy, codex's compaction tests) plus concrete LOC sizing of every borrow source, firming up the roadmap's effort estimates. Extends passes [02](02-pi-and-fabric-deep-dive.md)–[10](10-self-audit-and-alignment.md). Nothing here is implemented.

---

## Y. Upstream's verifier patterns

### Y.1 Durable: conformance suites as library code

`pi/packages/durable/src/testing/`: `storage-conformance.ts` and `env-conformance.ts` are **runnable conformance batteries over the interfaces themselves** (storage: read/write/scan/transaction semantics; env: dirs, watchers, file-change events), plus `storage-benchmark.ts`, and — the key trick — a **runner-agnostic assertion adapter** (`createExpectAssertions(expect)`) so the same suites run under any Vitest/Jest-compatible `expect` without importing either framework.

Borrow: the pattern for prime-agent's `SessionManager`/storage seam — a portable conformance battery that **any future storage lane (item U.3's SQLite) must pass**, written once as library code, not per-engine test files. This is how the durable port de-risks its storage choice, and it is the same discipline as the repo's existing gate batteries, applied to an interface instead of release plumbing.

### Y.2 The eval harness: prompts *and reloads* as input

`pi/packages/evals/src/harness.ts` wraps `createAgentSession` behind the separate **`vitest-evals`** library (`createHarness`, `normalizeHarnessRun`, `attachHarnessRunToError`, `TranscriptEvent`, `UsageSummary`). The harness input type is `Array<{type: "prompt"; content} | {type: "reload"}>` — **evals can inject a session reload mid-run** — with per-run diagnostics (transcript events + usage summary) and explicit model selection.

The reload-in-the-input-sequence detail matters: reload-after-compaction is exactly where the audit found the fragility (summary placement, digest churn), and upstream's eval grammar covers it as a first-class step. Any prime-agent behavioral eval suite should copy that grammar.

### Y.3 The bench task anatomy

`pi-fabric/bench/tasks/` holds **real repo-derived tasks** (e.g. `superjson-error-stack-serialization`, `scc-bounded-memory-spilling` — actual upstream bug classes). Each task ships a `verify.sh` that accumulates **named mechanical checks** (`check build 1; check <criterion> <0|1>`) in isolated `.verify-bin`/`.verify-out` dirs, checks derived mechanically from the prompt's stated acceptance criteria; `reward_partial` is the fraction of named checks passed. The two-file convention (task prompt + verify.sh) is all a prime-agent bench task needs.

### Y.4 Codex tests the real prompt, not a mock

`codex-rs/core/tests/suite/compact.rs` imports the **actual** `SUMMARIZATION_PROMPT`/`SUMMARY_PREFIX` constants and drives compaction through scripted protocol events (`TurnInputRequest`, `RolloutItem`, full `codex_protocol` types). No canned summaries. This is the shape finding 14 demanded: script the session events, run the real prompt, assert on the summary's structure. Prime-agent's LLM-engine eval (roadmap Phase 4) should follow: the summarization prompt is a tested public constant, and the eval feeds real event sequences through it.

---

## Z. Sizing — LOC of every borrow source

| Borrow source | TS/Rust LOC | Port implication |
|---|---|---|
| `repeat-guard.ts` (item 8) | **33** | hours, including tests |
| `output-budget.ts` (34) | 78 | hours |
| `orphan-repair.ts` (3) | 118 | hours |
| `cache-stats.ts` (10) | 174 | a day with telemetry |
| `budget-ledger.ts` (12) | 215 | a day |
| `auto-approval-classifier.ts` (20) | 396 | days — but needs item 45 (classifiers) first |
| `cache-warmer.ts` (1) | **453** | the biggest single-file port; plus `ModelPromptCache` catalog plumbing and the paint-latency test |
| `terminal-image.ts` (26) | 763 | days — two protocols plus capability detection |
| fabric `compaction/` module (items 3/9/…) | 4,466 | pick pieces; the whole module is a subsystem |
| fabric `prewalk/` | 1,923 | subsystem |
| pi `mcp` package (Streamable HTTP) | 3,244 | the transport slice only, not the package |
| pi `evals` package (13) | 3,329 | the harness + a Rust runner; suites are authored fresh |
| codex `apply-patch` (6) | 4,861 | vendor-or-adapt decision |
| codex `http-client` (A02) | 11,290 | patterns, not a wholesale port |
| pi `durable` package (strategic) | **22,842** | an ecosystem, not a port — confirms it sits in roadmap Phase 5 behind a state-layer decision |

Reading: **Phase 1 of the roadmap is nearly free** — its biggest items (repeat-guard, output budgets, orphan repair, failure-progress, the summarizer no-cache fix) sum to under 500 source lines. The cache warmer is one focused 453-line port plus catalog plumbing. Everything above 4k LOC is a subsystem that the roadmap already gates behind prerequisites. The sizing validates the phasing.

---

## AA. Ranked addition

57. **Conformance-suite pattern** — portable interface batteries (storage/env) with runner-agnostic assertion adapters; the contract any new storage lane must pass (pa-core test infra; supports items U.3 and the durable direction)

The roadmap's effort column can now be read against real numbers: Phase 1 ≈ small; Phase 2's centerpiece (cache warmer) ≈ one file; Phase 5's durable ≈ an ecosystem. **Survey depth is now at the source-of-the-source: how the borrowed machinery is itself verified.**
