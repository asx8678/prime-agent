# Pass 7: the compat surface, classifiers, certification, and skill patterns

Seventh survey pass, 2026-10-11 — the substrate layers: the model compat-field inventory (TS vs Rust), the classifier API, the durable spec, fabric's orchestration skill patterns, external-CLI interop, the certification harness, and the audit-trace envelope. Extends passes [02](02-pi-and-fabric-deep-dive.md)–[06](06-bash-ansi-workflows-and-grants.md). Nothing here is implemented.

---

## O. pi (TS upstream)

### O.1 The compat-field delta — the substrate every wire borrow needs

TS `pi/packages/ai/src` carries **25+ model compat flags**; Rust pa-types has ~14 (`supports_service_tier`, `supports_thinking`, `supports_fast_mode`, `supports_long_cache_retention`, `supports_usage_in_streaming`, `supports_strict_mode`, `supports_store`, `supports_reasoning_effort`, `supports_eager_tool_input_streaming`, `supports_developer_role`, `force_distillable_text`, `allow_sigint`, `allow_fallbacks`, …). TS flags with **no Rust counterpart**:

| TS flag | Gates | Feeds borrow item |
|---|---|---|
| `supportsMidConvoToolAdditions` / `supportsMidConvoToolChanges` / `supportsMidConvoSystemMessages` / `supportsMidConvoEffort` | the mid-conversation machinery | 25 (pass 4 §F.2) |
| `supportsToolSearch` (17 uses) / `supportsAdditionalTools` | deferred tools, tool discovery | 25, 36 |
| `supportsThinkingTokenBudget` | per-request thinking budgets | 32 (pass 5 §J.1) |
| `supportsExplicitPromptCacheMode` | explicit cache-mode requests | 1–2 (README §2) |
| `supportsFinishReason`, `supportsMaxOutputTokens`, `supportsOpenAIGrammarTools`, `supportsGoogleStrictToolSampling`, `supportsImages`, `supportsStrictTools` | per-provider wire shaping | various |

Porting the flags is the prerequisite layer: items 25/32/36 all need their gate present in `pa-types` before the wire work lands.

### O.2 The classifier API

TS `model-runtime.ts` composes a first-class **classifier surface** — `ClassifierApi`, `ClassifierModel`, `ClassifierOptions`, `ClassifierResult`, `getAvailableOfType("classifier")` — the machinery behind fabric's auto-approval classifier, extractive-history selection, and Jev routes. Rust pa-ai has no classifier model type. Borrow: classifier model support in pa-ai (a model kind + a `classify` entry point) — it unblocks items 20 (auto-approval), the extractive-history design (pass 2 §B.2), and any foreman-style observer.

### O.3 The durable spec ("Pico5")

`pi/packages/durable/docs/spec.md` — the normative durable-harness specification, typed against chord's `ReplicatedState`/`Draft`/`Op` and pi-ai's `DeferredHandle`/`ToolReference`. This is the reference document for the durable direction (pass 2 §A.2): read it before designing any Rust-side durability, not after.

---

## P. pi-fabric

### P.1 The fifteen orchestration skill patterns

`skillsets/typescript/`: `fabric-advisor`, `fabric-ambient`, `fabric-council`, `fabric-exec`, `fabric-foreman`, `fabric-fusion`, `fabric-graph`, `fabric-guide`, `fabric-jev`, `fabric-rlm`, `fabric-schema`, `fabric-spec`, `fabric-supervisor`, `fabric-swarm`, `fabric-workflow`. These are **model-facing skill files** (SKILL.md + code) implementing proven orchestration shapes over the kernel — e.g. `fabric-council` runs a bounded 3–5 role fan-out ("roles that disagree usefully") with a typed `CouncilOutcome` and workflow progress surfaces. Prime-agent already has the skill machinery — these patterns port nearly as-is onto the RLM kernel (`rlm.spawn` + `collect`), giving the product council/swarm/supervisor/advisor patterns without new runtime code.

### P.2 External CLI agent adapters

`src/agents/claude-cli.ts`: drives the Claude CLI as an external agent — cross-spawn with node-script detection, **model discovery with a 10 s timeout and 2 MB output cap**, thinking-level mapping, image passthrough. (codex-rs has an `external-agent-migration` crate in the same direction.) Borrow: external-CLI adapters so `rlm.spawn` can run other agents (claude, codex) as children — one child-runner among the kernels.

### P.3 The certification harness

`docs/certification.md`: three evaluation commands — `certify:context` (deterministic, offline, free), `certify:entropy` (metric v3 = `invocationRejections / actionOperations`, static normal-form preservation, deterministic compilation, identical-plan convergence, canonical idempotence, ambiguity refusal, forged-plan/schema-drift rejection, store round-trips), and `benchmark:real-resume` (opt-in, billable, **safe skip as the default behavior**). This is how fabric certifies its compaction/memory machinery deterministically. Borrow: certification commands for prime-agent's structural compaction engine — the deterministic engine (finding 03's default) is exactly the kind of component this harness pattern certifies: idempotence, fold-equivalence, refusal-on-ambiguity, drift rejection.

### P.4 The audit-trace V1 envelope

`docs/audit-trace.md`: every `fabric_exec` result persists as a **bounded durable envelope** (serialized details ≤ 512 KiB) holding `success/trace/audits/phases/error`; the **privacy-projected trace is the functional record for compaction, memory, and tool ownership**; audits persist verbatim with full arguments and previews; and — the discipline line — **"final session JSONL details never contain logs, return values, type errors, media payloads."** The structural record feeds the context; full data lives in the bounded envelope. Borrow: this split for prime-agent's kernel execution rows — it is the systematic answer to the session-log bloat our token audit measured by hand (whole tool outputs riding in history), and it composes with the audit-trail (`src/audit/`).

---

## Q. Ranked additions (45–50)

45. **Classifier model support in pa-ai** — unblocks auto-approval (20), extractive-history, and observers (pa-ai)
46. **The fifteen orchestration skill patterns** — council/swarm/supervisor/advisor as RLM-kernel skills (pa-core skills)
47. **Compat-field delta port** — the mid-convo/tool-search/thinking-budget flags; the substrate for items 25/32/36 (pa-types)
48. **Audit-trace envelope discipline** — bounded execution envelopes; session JSONL keeps structure only (pa-core/pa-daemon)
49. **Certification harness** — deterministic offline certification of the structural compaction engine (new tooling)
50. **External CLI agent adapters** — claude/codex as `rlm.spawn` children (pa-daemon)

Item 47 is deliberately unglamorous: it is the prerequisite layer that makes items 25, 32, and 36 landable. Item 48 answers the session-log bloat the token audit measured (finding 08's 79 KB file inserts) with a structural rule instead of per-case caps.
