# Pass 19: codex, properly — the architecture crates

Nineteenth pass, 2026-10-11 — codex only ever had the broad pass-1 sweep plus safety kernels; this pass opens the architecture crates: the context manager, session/turn model, MCP prewarming, rollout compression, the code-mode PTC, skills manifests, plugins, and the app-server protocol. Extends [02](02-pi-and-fabric-deep-dive.md)–[18](18-context-and-caching.md). Nothing here is implemented.

---

## AZ. Codex's context architecture

### AZ.1 History = bounded host-owned facts + the parent model window

`core/src/context_manager/history.rs` is the deepest piece. Codex's history is not just messages: **"parent model history and bounded host-owned context facts"**. Compaction *replaces the model window* and can activate **thread-owned Guardian review**; snapshots carry reviewer policy and retained facts **atomically**; **checkpoint replay and source-call rollback share their live lifecycle**. Two details stand out:

- **"Token estimates charge item content rather than transport metadata"** — the estimate basis is what the items say, not serialized size. A better foundation than either chars/4 or 3.5 chars/token (item 31): item-content-aware estimation.
- **Legacy recovery discipline**: oversized instructions keep incomplete excerpts for bounded root review, including sources recovered from legacy Guardian checkpoints *before their raw history is dropped* — nothing is destroyed before its salvageable content is extracted. A `history_user_authorization` submodule gates history contents.

### AZ.2 Dual-scope window tracking

`session/context_window.rs`: `ContextWindowTokenStatus` tracks **`active_context_tokens` (the full window) separately from `auto_compact_scope_tokens` counted against a configurable `model_auto_compact_token_limit` with a scope enum** — compaction can be tuned per model at a custom limit independent of the raw window, with prefill accounting and explicit `…_reached` flags. This is the per-model compaction knob both fabric (per-model threshold maps) and prime-agent (single threshold) approximate; codex ships it as a first-class setting.

### AZ.3 MCP prewarming with coalescing

`session/mcp_prewarm.rs`: a **bounded channel coalesces refresh requests; the worker only prepares the newest thread state; "exact model steps remain the correctness path."** Two distinct invalidations: `invalidate_resource_caches()` (plugin changed — connections reusable, skill resources not) vs `reprojection` (contributor state changed — caches kept). Mid-session MCP tool refresh without per-request stalls, and the correctness never depends on the prewarm.

### AZ.4 Rollout compression

`rollout/src/compression.rs`: transparent **file-level compression of persisted sessions** — blocking reader, error metrics, permission handling, atomic counters. Prime-agent's session JSONL grows unbounded; this is the storage-side answer (pairs with item 48's context-side envelope hygiene).

### AZ.5 The code-mode PTC: cells as actors

`code-mode-runtime/src/service.rs`: cells with **execution limits** (`CodeModeSessionCellExecutionLimits`), **time-slicing** (`DEFAULT_EXEC_YIELD_TIME_MS`), **pending cells** — `ExecuteToPendingOutcome`/`WaitToPendingOutcome` — a cell can yield and be waited on later, plus delegates and per-request image detail. This is codex's ipython-equivalent with actor-shaped cells: the same shape as prime-agent's kernel but with explicit yield/wait semantics and bounded execution.

### AZ.6 Skills as manifests

`skills/src/interface.rs`: skills declare themselves via **`agents/openai.yaml`** — `display_name`, `short_description` (≤ 1,024 chars), **icons and a brand color**, names ≤ 64 chars. A manifest-based skill contract with UI metadata, complementary to SKILL.md bodies — the catalog side of prime-agent's skill discovery (finding 06's fix could use bounded manifest fields, not full descriptions).

### AZ.7 The rest of the map

- **`plugin/`**: manifest + provider + bundled_hooks — a plugin system with declared providers (the codex counterpart of the extension-seam decision, item 18).
- **`app-server-protocol/`**: **precomputed typed exports** (`precomputed_exports.rs` — the wire protocol as generated, schema-fixtured types) + `export_user_verification` + rpc — the IDE/app surface; prime-agent's ACP is the pi lineage, this is the codex lineage, both worth knowing.
- **`session/`** also holds: `daemon_recovery.rs` (crash recovery), `guardian_checkpoint.rs` (review checkpoints tied to compaction), `inject.rs`/`input_queue.rs` (steering injection with queues), `config_refresh.rs` (mid-session config changes), `extension_interruption.rs`.

---

## BA. Ranked additions (66–70)

66. **Content-based token estimates** — charge item content, not transport metadata; refines item 31's calibration question at the estimate basis (codex) — Phase 1
67. **Per-model auto-compact token limit scopes** — `model_auto_compact_token_limit` + scope, independent of the raw window; complements fabric's per-model thresholds (codex) — Phase 1
68. **MCP prewarm coalescing** — bounded channel, invalidate-vs-reproject distinction, correctness never depends on prewarm (codex) — Phase 2
69. **Rollout/session-log compression** — transparent file compression with error metrics; pairs with 48 (codex) — Phase 5
70. **YAML skill manifests** — bounded fields + UI metadata for the skill catalog (codex) — Phase 4, pairs with 25/36

Seventy items. Codex turns out to hold the per-model compaction knob, the better estimate basis, the coalesced MCP refresh pattern, and the storage-side compression — all Phase-1/2-sized except compression, and all previously unmined after eighteen pi/fabric-focused passes.
