# Pass 20: the codex → prime-agent fit map

Twentieth pass, 2026-10-11 — the fit question answered crate by crate: what from codex lands in prime-agent cleanly, what fits with caveats, and what does not. Grounded in the sandboxing stack, hooks engine, connectors, exec, and message-board crates opened for this pass. Extends [19](19-codex-deep-dive.md). Nothing here is implemented.

---

## The structural advantage

Codex is the only **Rust-to-Rust** source in the survey: borrows avoid the TS→Rust translation cost entirely, and several crates are vendor-or-adapt candidates rather than ports (license check below). That makes codex the mechanical-borrow source even where pi/fabric supply the design.

## BB. Fits cleanly — ranked by fit × value

| Codex | → prime-agent | Verdict | Effort |
|---|---|---|---|
| `apply-patch/` (Hunk parser, **seek_sequence fuzzy matching**, streaming parser) | pa-core tools (edit path) | **High** — battle-tested Rust patching; vendor or adapt behind the existing edit surface | medium (4,861 LOC, adapt) |
| **hooks engine** (`hooks/`: declarations, config rules, registry, schema, **output spill to files**, MCP hooks, legacy notify; Pre/PostCompact hooks in core) | pa-agent/pa-core | **High** — prime-agent has only the stop-hook; codex's engine is the extension point the kernel lacks (item 71, new) | medium |
| Per-model auto-compact scopes (`model_auto_compact_token_limit` + scope enum, dual window tracking) | pa-core compaction | **High** — first-class per-model knob; small (item 67) | small |
| Content-based token estimates ("charge item content, not transport metadata") | pa-core compaction/estimate | **High** — better than both chars/4 and 3.5 (item 66) | small |
| Cache-discontinuity classifier (field-diff → named bust reason) | pa-ai/pa-telemetry | **High** — pure derivation, tiny (item 65) | small |
| MCP prewarm coalescing (bounded channel, invalidate-vs-reproject) | pa-core mcp | **High** (item 68) | small-medium |
| Retained-tool-output budgets + compact model-fallback ladder | pa-core compaction | **High** (items 4–5) | small |
| Rollout compression (transparent file compression, error metrics) | pa-core session store | **High** — storage-side answer to unbounded JSONL (item 69) | medium |
| YAML skill manifests (bounded fields + UI metadata) | pa-core skills catalog | **High** — catalog side of finding 06's fix (item 70) | small |
| execpolicy command DSL + trusted-commands shape | pa-core tools / kernel approvals | **Medium-high** — the policy layer before full sandboxing (item 60 + pass 14 §AI.1) | medium |
| Guardian isolated reviewer | pa-daemon | **Medium-high** — escalation tier above the classifier (item 59) | medium |
| exec event processors (human vs JSONL separation) | pa-cli print/rpc modes | **Medium** — pattern fit for output lanes | small |
| app-server precomputed typed exports | pa-daemon ACP/wire | **Medium** — reference for typed wire surface; ACP remains the pi lineage | pattern only |
| agent-message-board (client/notifications/protocol) | pa-daemon rlm | **Medium** — matches agent_message.send + completion inboxes; reference | pattern |
| memories read/write pipeline (injection, citations, usage telemetry, diff-before-persist) | Continual Harness | **Medium** — design-level fit (pass 13 §AG) | large |

## BC. Fits with caveats

- **`linux-sandbox`** — `no_new_privs` + **seccomp** in-process, **bubblewrap** for filesystem isolation (bundled + bazel bwrap modules, daemon/fd mounts, a launcher binary). The most complete piece of the biggest prime-agent gap. **Caveat: Linux-only.** prime-agent's own next-steps notes the dev environment is macOS, and codex ships no macOS sandbox crate — adopting this gives prime-agent a Linux sandbox story while leaving macOS to a seatbelt-based effort that does not exist in any surveyed source. Strategic, phased: Linux first.
- **TUI** — both ratatui; pa-tui is parity-tested against TS, so borrow patterns only (if anything) — low priority.
- **connectors** (`app_tool_policy`, metadata stores, directory caching) — overlaps MCP; the per-app **tool policy** piece is the interesting residue.
- **network-proxy / responses-api-proxy** — pattern source for A02 hardening (already item 5); pa-agent's proxy is a thin client today.
- **config-schema** — JSON-schema generation for settings, matching TS's published configuration schemas; nice-to-have beside item 22's settings delta.
- **file-watcher** — a single-module watch service; check against pa-core's existing fs-watch before borrowing.

## BD. Does not fit

- `cloud-tasks`/`cloud-client`/`backend-client`/`chatgpt` — OpenAI-cloud-specific.
- `realtime-webrtc`/`voice-host` — voice, out of scope.
- `lmstudio`/`ollama` — local-model providers; prime-agent's openai-compat lane + catalog covers these as configuration, not crates.
- Bazel build infra, `workload-identity`, `v8-poc` — build/experimental scaffolding.
- The Bend-grade formal machinery has no codex counterpart (that lane stays fabric-only).

## License note

Codex is Apache-2.0 (LICENSE at repo root) — compatible with vendoring/adapting crates into prime-agent with attribution; the Rust-to-Rust borrows in §BB are license-clean. Verify NOTICE files when actually vendoring.

---

## Ranked addition

71. **Hooks engine** — declarations, config rules, registry, schema, output spill, MCP + compact Pre/Post hooks; the general extension point prime-agent's agent loop lacks (pa-agent/pa-core, Phase 2)

Seventy-one items. The fit map's one-line summary: **codex supplies the mechanics** (patching, hooks, policy, compression, window scopes, estimates), **pi supplies the wire and product shape**, **fabric supplies the orchestration and verification designs** — and the overlap is small enough that nearly all seventy-one items survive triage.

---

## Fit map v2 (pass 21 — the remaining crates)

- **The sandbox matrix, complete.** Pass 20's caveat stands but the matrix is richer than stated: `sandboxing/` is a full **manager crate — bwrap + landlock + Linux PID namespaces + policy transforms** (`landlock.rs`, `linux_pid_namespace.rs`, `manager.rs`, `policy_transforms.rs`) layered over the `linux-sandbox` helper's seccomp; `mxc-sandbox` is the **Windows** story ("native Windows MXC helper… never enters MXC's ACL fallbacks; policy conversion is portable"). So: Linux gets *three* mechanisms behind one manager (item 72, new — the crate prime-agent would actually adapt), Windows gets MXC, and **macOS remains absent from every surveyed source** — that effort is greenfield wherever it lands.
- **The Guardian implementation shape** (grounds item 59): reviewer sessions are **pooled and prewarmed** — `prepare_review_prewarm` + `PreparedGuardianContext` bind captured parent action/environments/authorization/context snapshots, a `ReviewContextPolicy` governs what the reviewer sees, and `run_guardian_review_session` executes synchronously with analytics. The escalation tier is a **warm pool with prepared contexts**, not a cold classifier per call — that's the shape prime-agent's guardian tier should copy.
- **`thread-store`** — a trait with `in_memory` and `local` backends plus `live_thread`: the same conformance-store shape as pi durable's testing batteries (item 57), cross-confirmed in a second source; `message-history` adds batched history operations.
- **Small fits**: `stdio-to-uds` (the stdio↔UDS relay — the MCP/IDE bridge pattern for the daemon socket), `agent-roles` (role config + discovery + loader) and `agent-identity` (a small identity type) mapping to RLM child identity, and `process-hardening` (single-module hardening utilities).

Fit map now covers every codex crate worth triage; the no-fit list from §BD is unchanged.

## Fit map v3 (pass 22 — the final sweep)

- **`worktree/`** — a full Rust worktree-management crate (`apply`, `git`, `metadata`, `paths`, `settings` over git-utils): the Rust-to-Rust grounding for item 15's worktree-per-subagent isolation, no TS translation needed (item 73).
- **`external-agent-migration/`** — "migration helpers for importing external-agent configuration into Codex": `hooks_cla` (Claude hooks), `hooks_cur` (Cursor), `hooks_common`, `mcp`, `memory`, `memory_import`, `detect`. An onboarding lane that imports Claude/Cursor users' hooks, MCP servers, and memory — item 74; complements item 50 (running external agents as children) from the intake direction.
- **`git-utils/`** — typed git operations (`status`, `baseline`, `branch`, `apply`, `fsmonitor`, **`trust`**): medium fit — pa-core's bash covers git, but a typed layer with fsmonitor and trust would serve review diffs and worktree flows.
- Small/already-covered: `file-system` (find-up — pa-core has discovery), `secrets` + `keyring-store` (small, careful error taxonomy), `agent-graph-store` (a third thread-store-shaped local store — item 57 now triple-confirmed), `collaboration-mode-templates`, `stdio-to-uds` (v2).

Codex triage is closed at **74 items**.
