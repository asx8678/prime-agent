# Pass 8: the substrate — chord, tool entropy, and the agent-facing UX

Eighth survey pass, 2026-10-11 — the composition substrate (chord), the model-schema extras, fabric's tool-entropy/normal-form system, and the interface/UX layer (focused conversations, task inspector, code previews). Extends passes [02](02-pi-and-fabric-deep-dive.md)–[07](07-compat-certification-and-skills.md). Nothing here is implemented.

---

## R. pi (TS upstream)

### R.1 Chord — the composition runtime under everything

`pi/packages/chord/` (README + src: api, bundler, context, **delta**, **facets**, json, node, **services**): plugins are synchronous setup units that **declare the services they provide and require**; a host then validates the complete dependency graph, binds services, **activates providers before consumers**, and disposes in reverse order. **Facets** split a plugin into per-environment pieces (backend, browser, TUI) that load where they belong; **services** are typed, stable tokens, singleton or keyed, process-local or remote.

For prime-agent this is the design substrate for the extension seam (pass 3 §C.1): when the Rust extension surface is built, the facet/service/DAG-validation model is what to steal rather than inventing a loader-only design. It is also the state layer under the durable harness (pass 8 §S.5).

### R.2 Model-schema extras

`pi/packages/ai/src/types.ts`: models carry a **preferred transport** plus **WebSocket transport support** ("connection/open handshake" scoped) — Rust providers are HTTP/SSE only. `thinkingBudgets` per model and `promptCache` tiers (item 1) complete the picture. Borrow: a WebSocket transport lane is a future provider capability that arrives with the fleet ports (items 11, 44). The TS `modes/` (interactive, rpc, json-event, print) and the TUI component set (including an `image.ts` widget — consistent with item 26) match the Rust surface.

---

## S. pi-fabric

### S.1 Tool entropy and normal forms — model-ergonomics as a system

`docs/entropy.md` + `src/entropy/`: fabric **measures invocation friction** (metric v3: `invocationRejections / actionOperations`) and **statically compiles bounded argument normalizations** — with hard constraints: capabilities are never shrunk to observed usage; compilation and certification are deterministic and offline (no LLM calls); **"an unprovable candidate does nothing"**; static proof means checking a host-authored compatibility language against the live schema.

This is a category the whole survey hadn't named: **systematic model-ergonomics** — measure where models fail to call tools correctly, then compile *provably safe* call-shape repairs. For prime-agent: friction telemetry per kernel tool/argument + a compiled-normalization lane, composing with the repeat-guard (item 8) and the repairs pattern (pass 2 §B.6).

### S.2 Focused conversations — the RLM observability UX

`docs/interface.md`: **`/fabric chat <id>`** opens a full-screen conversation with *any* agent or actor child — "opening, navigating, and closing the view never calls Pi's session-replacement API or starts another worker." Lineage badges (`Main > child > grandchild`), read-only badges for finished children, **per-participant run-scoped token/cache/cost statistics** ("Main's statistics are never substituted"), drafts and scroll positions surviving target switches, and a native-quality transcript with full tool details.

This is the highest-value UX borrow for prime-agent's RLM surface: pa-tui currently shows subagents through the agents_view roster; a focused-conversation view over `rlm.collect`'s transcript data with per-child cost stats would make parallel children first-class. Pairs with the budget ledger (item 12) for the stats source.

### S.3 The background task inspector

`/fabric tasks` (Ctrl+Alt+T): live view of every detached command — ID, purpose, elapsed, access hint, bounded output tail, cwd, PID, exit status, output path, **time-since-last-output shown separately from elapsed**, monitor delivery/deadline; `x` twice stops; completion hints last 30 s; retained jobs stay inspectable. Borrow: a pa-tui task inspector over the kernel's detached bash jobs (the mechanics exist; the inspectable surface doesn't).

### S.4 The code-preview vocabulary

Nested `pi.read/bash/grep/write/edit` calls share one presentation vocabulary: offset-aware read gutters, **syntax and secret warnings**, grouped grep matches, iconized path trees, edit diffs with word-level emphasis; an **execution-wide throttle coalesces parallel nested-call updates** so continuous streams aren't starved; long ANSI rows are bounded without clearing the enclosing background. Borrow: **secret warnings in tool previews** (scan rendered tool output for credential shapes before display) and the coalescing-throttle pattern for parallel kernel calls.

### S.5 Durable harness internals — and the jev correction

`pi/packages/durable/src/harness/harness.ts`: chord replicated state + abort contexts; `SessionImpl` with **contained transactions**; a `TaskScheduler` with `InvocationBinding`s; conversations with ownership, cursors, and pages; a document model (`InboxDoc`, `LiveDoc`, `ProviderDoc`, `AgentDoc`); `createCompaction`; `readContext`; a `BUILTIN_TASKS` registry. This confirms the sequencing: a Rust durable port needs a **state-layer decision first** (fabric's `docs/state-layer.md` is the reference), then the scheduler/documents on top.

Correction: pass 3's "`src/jev/` appeared empty" was an `ls` artifact — the directory holds `auth.ts`, `client.ts`, `config.ts`, `decision-profiles.ts`, `decision-schema.ts` (+ manager/observation elsewhere), as described in pass 3 §D.1.

Also noted: fabric's TS kernel runs **two API profiles** (`executor.codemodeProfile: "additive" | "native"`) through one registry, approvals, and audit path (`docs/codemode-parity.md`) — a compatibility pattern for any future dual-lane kernel.

---

## T. Ranked additions (51–56)

51. **Focused-conversation views** — chat with any `rlm` child, per-child token/cache/cost stats, no worker disruption (pa-tui, rides item 12)
52. **Chord's facet/service model** — the design substrate for the extension seam, item 18 (pa-cli/pa-core)
53. **Tool entropy + safe normalizations** — friction metrics with provably-safe compiled repairs (kernel telemetry)
54. **Background task inspector** — live detached-command surface for pa-tui (pa-tui)
55. **Secret warnings in tool previews** — credential-shape scan on rendered output (pa-tui)
56. **WebSocket transport lane** — per-model transport support, with the fleet ports (pa-ai)

Item 51 is the RLM observability gap pa-tui doesn't yet cover; item 53 names a new category (model-ergonomics) that composes with the repeat-guard and repairs; item 55 is a small, self-contained safety win.
