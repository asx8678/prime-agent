# Pass 4: terminal rendering, cache-preserving wire changes, and verified state

Fourth survey pass, 2026-10-11 — the layers still unexplored after [02](02-pi-and-fabric-deep-dive.md) and [03](03-extensions-agents-and-verification.md): the TS TUI's native surface, pi-ai's streaming/estimation utils, the experimental worker architecture, and fabric's state-layer/verified-kernels design docs. Nothing here is implemented.

---

## F. pi (TS upstream)

### F.1 Terminal image rendering — pa-tui has none

`pi/packages/tui/src/terminal-image.ts` implements **kitty and iterm2 image protocols** ("kitty" | "iterm2" | null capability detection, cell-dimension math, aspect preservation, `imageId` for reuse/replacement in place, the `imageWidthCells` setting), plus a per-platform **NAPI native module** (`tui/native/{darwin,linux,win32}`, clipboard.h). Rust pa-tui has hyperlinks but no image protocols (verified) — `attach_image` attachments render as text only. Borrow: inline image display for pa-tui (kitty + iterm2), capability detection, and image-ID reuse. Medium effort, high UX value for the multimodal path.

### F.2 Mid-conversation tool additions — the cache-preserving way to lazy-load tools

`pi/packages/ai/src/api/anthropic-messages.ts:1136-1351` + `utils/text.ts:28`: with the `supportsMidConvoToolChanges` compat, the **request-level tool list stays fixed** ("so the cached prefix survives"), and later tools are defined **by value inside `tool_addition` blocks** mid-conversation, withdrawn via `tool_removal`, with same-name redefinitions expressed as additions. System-message **updates** render through `renderSystemMessageUpdate` and are held back to be emitted directly before the next assistant message.

This is the TS-proven wire mechanism for exactly what `findings/tokens-and-context/06` wanted (skills always loaded → on-demand) **without busting the prompt cache**: a compact tool registry up front, skills/tools added by value when needed. Compat-gated per model. Borrow into pa-ai (wire) + pa-core (lazy skill materialization). Medium-high effort, high value.

### F.3 pi-ai streaming and estimation utils

- **`utils/json-parse.ts`** — `parseJsonWithRepair`/`parseStreamingJson` on the `partial-json` library with control-character escaping rules: streaming tool arguments and structured output parse incrementally instead of failing. Rust pa-ai parses complete frames only — borrow the partial-JSON repair for streaming constrained output.
- **`utils/overflow.ts`** — a curated, example-documented regex library of **eight provider overflow-error forms** (Anthropic ×2, OpenAI ×2, OpenAI-compatible/LiteLLM, Google, xAI, Groq). Rust's `is_context_overflow_failure` covers fewer forms — worth diffing against for recovery-path coverage.
- **`utils/estimate.ts`** — `CHARS_PER_TOKEN = 3.5` with the same usage-anchor + trailing split prime-agent uses. **Rust uses chars/4 (÷4)**: ~14 % more conservative, so the Rust compaction trigger fires slightly early. Not a bug — but a calibration divergence worth recording with the T01/T02 notes.
- `oauth-page.ts` (local OAuth landing page), `assistant-message-frame.ts`, `node-http-proxy.ts`, `diagnostics.ts` — small robustness surfaces.

### F.4 The experimental worker architecture

`pi/packages/coding-agent/src/experimental/` — coordinator + **session-worker-manager** + session-catalog + session-worker, built on **chord** services (`ServiceCall`/`ServiceProviderUpdate`, `BACKGROUND_CONTEXT`/`TODO_CONTEXT`) and **pi-server** routed session attachments, with worker control via env tokens. This is the TS next-gen daemon shape; prime-agent's pa-daemon (supervisor + worker per session) is already the same direction. Borrow as it stabilizes: the **session catalog** and the **worker control-plane** patterns; keep the `experimental/durable` + plugin/service seams in view for the extension-system decision (pass 3 §C.1).

### F.5 TUI polish pack

`kill-ring.ts` (emacs-style editor kill ring), `fuzzy.ts` (fuzzy autocomplete matching), `oklab.ts` (perceptual color space), `latex.ts`, `alt-screen-search.ts`, `stdin-buffer.ts` — small, self-contained pa-tui lifts behind settings.

---

## G. pi-fabric

### G.1 `docs/state-layer.md` — claims, evidence, certification

A typed world-model layer: **claims** (transition summary + labeled state move), **evidence attachment** (replayable shell commands stored on the transition — "attachment means the commands can be replayed; it says nothing about whether they ran"), and **certification** (`state.verify` runs selected evidence; certified only when every result exits 0), with **compare-and-swap persistence** of the current certificate and **fail-closed verification** — any missing target, empty evidence, non-zero exit, spawn error, timeout, cancellation, or publish failure yields `certified: false` with blocking reasons, and a publish failure CAS-revokes the old certificate first.

Borrow: this is the "verifiers over self-assessment" repo rule as a kernel feature — the natural verification layer for prime-agent's `--goal` machinery (and the acceptance-ledger discipline this session used by hand). "Passing tests count as evidence. They do not prove the claim" is the right epistemic line.

### G.2 `docs/verified-kernels.md` — proven policy kernels

Fabric compiles acceptance points and state transitions from **Bend specification kernels** (`LAWS.bend`, `PROOF.bend` with closed proofs): production calls the generated code, **no handwritten fallback**, and "independent predicates prevent a change to the kernel's condition builder from silently changing its law." The Bend tooling is out of scope for prime-agent, but the pattern is not: **spec-independent predicates + an acceptance ledger** is exactly how the structural-compaction invariants (`folding_a_saved_state_further_equals_folding_everything`, the byte bounds) should be pinned — the laws live beside the code and review them independently.

### G.3 Remaining modules

- `durable/` (agent, session, storage, worker-host, worker) — fabric's own worker host over pi sessions.
- `programs/` (host, source, store) — persistent program management (the Jev supervisor substrate).
- `residency/` (launcher, pi-entry, protocol, actor-client) — long-lived process hosting.
- `audit/` (assessment, details, projection, trace) — the audit trail behind `docs/audit-trace.md`.

---

## H. Ranked additions (extend the list to 31)

25. **Mid-conversation `tool_addition`** — cache-preserving lazy tools/skills; serves finding 06 (pa-ai + pa-core)
26. **Terminal images (kitty/iterm2)** for pa-tui — attach_image renders inline (pa-tui)
27. **State-layer verification semantics** — claims/evidence/certification for `--goal` (pa-core, kernel)
28. **`parseJsonWithRepair`** — partial-JSON streaming repair (pa-ai)
29. **Experimental worker patterns** — session catalog + control plane as pa-daemon evolves (pa-daemon)
30. **TUI polish pack** — kill-ring, fuzzy, oklab, latex (pa-tui)
31. **Overflow-pattern library + estimate calibration** — 8 documented provider forms; record the 3.5-vs-4 chars/token divergence (pa-ai/pa-core)

Pass-4 item 25 is the highest-value wire borrow since the cache warmer: it makes finding 06's on-demand skills fixable without re-opening finding 01.
