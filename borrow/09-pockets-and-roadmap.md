# Pass 9 (closing): the skills contract, sectioned system prompts, and the adoption roadmap

Ninth and closing pass, 2026-10-11 — the last pockets plus the piece the folder lacked: a dependency-ordered roadmap that sequences all 56 borrows into phases. Extends passes [02](02-pi-and-fabric-deep-dive.md)–[08](08-substrate-chord-entropy-ux.md). Nothing here is implemented.

---

## U. Final pockets

### U.1 Fabric's skills invocation contract — the upstream answer to finding 06

`pi-fabric/docs/skills.md`: a **core-first, user-opt-in skill hierarchy**. The model can invoke exactly **one execution reference** (`fabric-exec`); every advanced workflow declares `disable-model-invocation: true` and **stays out of the model catalog** — "only the selected execution reference spends always-on model context." A user-only router (`/skill:fabric-guide`) names one exact command and stops; agent policy forbids reading user-only skills autonomously.

And the **information hierarchy** for skill content: (1) `SKILL.md` carries required ordered actions and **checkable completion criteria**; (2) a **hard pointer** for material the run must load before execution; (3) a **branch pointer** for material some runs need; (4) a **soft pointer** for optional depth. Both TS and Python trees ship the same fifteen canonical names; only the selected tree is contributed through resource discovery.

This is finding 06's fix, already designed upstream: one compact always-on reference, everything else opt-in, with a three-tier pointer taxonomy replacing "full descriptions always loaded." Port the contract into pa-core's skill discovery.

### U.2 Sectioned system prompts with named updates

`pi/packages/ai/src/utils/text.ts:25-45` (`renderSystemMessageUpdate`): TS system prompts are **sectioned** — mid-conversation updates render as `Updated system prompt section "name": <value>` / `Removed system prompt section "name".` alongside free text. Combined with the mid-convo emission point (pass 4 §F.2), this means named sections of the system prompt can change mid-session **without rebuilding the whole prompt and without busting the cache prefix**. For prime-agent's dynamic tail (session-role, environment, skills), a sectioned system prompt + named-section updates is the principled fix behind finding 01's two-block proposal. Depends on item 47's compat flags.

### U.3 Durable storage: JSONL *and* SQLite

`pi/packages/durable/src/storage/` — both `jsonl/` and `sqlite/` engines. Rust sessions are JSONL-only (parity-pinned). A SQLite lane is the natural storage engine if/when the durable commit-before-show model (pass 2 §A.2) is adopted — it is also what makes contained transactions and task graphs cheap.

### U.4 Fabric's dispatch-plan kernel

`docs/provider-capabilities.md`: every registry action runs through a **proved dispatch-plan kernel** — capability views, tickets bound to provider binding UUID + generation + operation kind + action ref + descriptor hash + payload slot, consumed synchronously, re-described and authority-checked before dispatch. Far beyond prime-agent's needs today, but it is the rigorous end of the effect-policy idea (item 24): when prime-agent's kernel grows provider actions, the ticketed-dispatch pattern (bind → hash → consume → re-check) is the reference.

### U.5 `pi-manifest` — the package resource manifest

`pi/packages/coding-agent/src/core/pi-manifest.ts`: `PiManifest { extensions, skills, prompts, themes }` — packages declare their resources in a manifest, and discovery reads the declared fields. A small contract worth mirroring in prime-agent's package resource discovery (it already resolves skills from packages; the manifest makes the declaration explicit and cheap).

---

## V. The adoption roadmap — 56 items, dependency-ordered

Read the phases as: each phase's prerequisites come from earlier phases. Every user-visible feature carries the repo's standing rules: adoption telemetry in the same change, nothing user-visible blocking on network (paint-latency rule), and parity divergences flagged in the PR.

### Phase 0 — substrate (invisible, unlocks everything)
- **47** compat-flag port (mid-convo, tool-search, thinking-budget, explicit-cache flags) — prerequisite for 25/32/36/U.2
- **45** classifier model support in pa-ai — prerequisite for 20 and extractive-history
- Small parity: **41** non-vision image placeholders, **43** chunk-boundary ANSI split, **35** strict-schema filtering

### Phase 1 — token and cost (closes the audit's open mechanisms)
- **§0** summarizer/gate `CacheRetention::None` — the new defect, few lines
- **1+2** cache warmer + `ModelPromptCache` catalog tiers (+ **8** leases surface) — the T02 resolution; runs on a background worker, red-first hanging-sink test per the paint-latency rule
- **10** cache-miss attribution + **22** `showCacheMissNotices` — visibility for 01/02
- **8** repeat-guard + **38** failure-progress digest — finding 09 closed
- **34** failure-path output budgets; **3** orphan-repair; **9** carry list (lands `next-steps/04`)

### Phase 2 — history and wire
- **4** compaction model-fallback ladder (T05); **5** retained-tool-output budgets + **A02** idle timeouts
- **25+36** mid-conversation `tool_addition` + searchable tool catalog (needs Phase 0) — finding 06 closed without re-opening 01
- **U.2** sectioned system prompts with named updates — the principled form of finding 01's split
- **48** audit-trace envelope discipline for kernel rows

### Phase 3 — kernel and safety
- **20** auto-approval classifier with hard escalation (needs Phase 0's classifiers) — autonomous mode's missing safety layer
- **32** scoped `thinking.*` effort control; **12** budget ledger + **39** schema-validated agent results — finding 17 closed properly
- **15** worktree-per-subagent isolation; **53** tool-entropy friction telemetry + safe normalizations

### Phase 4 — verifiers and UX
- **19** DeepSWE-style bench with read-pathology stats + **49** certification harness for the deterministic compaction engine
- **26** terminal images; **51** focused-conversation views (per-child cost stats); **54** task inspector; **55** secret warnings; **30** TUI polish
- **46** the fifteen orchestration skill patterns on the RLM kernel; **U.1** the skills invocation contract

### Phase 5 — strategic
- **18** extension seam (chord facet/service model, **52**) — unblocks the fabric-class ecosystem
- Durable: commit-before-show + **U.3** SQLite lane + state-layer decision
- **11/33/44** provider fleet + `pi-messages` + OAuth flows; **56** WebSocket lane
- **50** external CLI adapters; **23** Jev decision runtime; **24** effect policies; **16** sPTC (needs a sandbox story); mesh/actors

**Cross-cutting corrections already recorded:** ANSI stripping verified present in Rust (pass 6 §L.1); queue modes verified ported (pass 3 §C.2); jev directory listing artifact fixed (pass 8 §S.5).

### Roadmap v2 updates (passes 10–13)

- **Sizing (pass 11):** Phase 1 sums to **< 500 source LOC** (repeat-guard 33, output-budget 78, orphan-repair 118, failure-progress, the summarizer no-cache fix); the cache warmer is one 453-line file plus catalog plumbing; durable is a 22.8k-line ecosystem — the phasing is validated by measurement, not judgment.
- **New items by phase:** **58** per-file mutation queue → Phase 1 (kernel edit/write safety, ~60 lines); **57** conformance-suite pattern → Phase 0/2 (the battery any new storage lane must pass); **59** guardian-pattern isolated reviewer → Phase 3 (the high-fidelity escalation tier above item 20's classifier); **60** execpolicy command DSL → Phase 5's sandboxing track.
- **Corrections' effects (pass 10):** item 30 (TUI polish) shrinks to latex + oklab only — kill-ring and fuzzy already exist in pa-tui.
- **Cost revisions (pass 12):** fabric's **Python skillset tree** materially lowers item 18's cost — hosting fabric's Python lane on prime-agent's Python kernel needs far less surface than the full TS extension API; treat it as the on-ramp for the extension-seam decision.
- **Item 3.8 grounded (pass 13):** codex `memories/` is a phased write pipeline (extract → artifact → diff helpers → pruning) plus a read path that injects memories as developer instructions, parses citations, and classifies read-usage telemetry — the concrete shape for the Continual Harness's memory endgame.

---

**The borrow survey is closed.** Nine passes, 56 ranked items plus five closing pockets, two upstream repos and the codex peer mapped at module level, three in-record corrections, and every open finding from the tokens/context audit now has an upstream design attached to it. The folder is a decision document: pick a phase, read the linked items, port.
