# Pass 14: design-doc closeout

Fourteenth pass, 2026-10-11 — the last unread design docs: fabric's schema-enforcement, component-calculus, harnesses, and programs; durable's design-evolution and handoff documents. Adds one item (61) and two recorded lessons. Extends passes [02](02-pi-and-fabric-deep-dive.md)–[13](13-provider-protocol-and-memory-pipeline.md). Nothing here is implemented.

---

## AI. pi-fabric

### AI.1 Schema enforcement — the practical command-allowlist shape

`docs/schema-enforcement.md`: an opt-in host authorization + local-file transaction layer. `schema.mode: "enforce"` carries `certificateTtlMs`, `maxFiles`/`maxBytes` bounds, and **`trustedCommands`** — named commands declared as explicit argv (`command`, `args`, `shell: false`, `timeoutMs`): no shell interpolation, named in config, individually timeout-bounded. Combined with state-layer.md's rule that enforce mode blocks model-originated `state.transition/verify/goal` calls, this is the *practical* form of items 37/60: **trusted-commands as explicit argv** is the format prime-agent's kernel could adopt without building a policy DSL first.

### AI.2 Saved programs — content-addressed, authority-free

`docs/programs.md`: a saved program (fabric_exec or Jev) stored **by name + content digest** (`<sha256>.json` + `index.json` + a `.lock` reaped after 30 s if a writer crashed), runnable nested (`programs.run`) or host-triggered without the model (`/fabric run`, a versioned event). The safety properties are the borrow: *"they do not schedule themselves, they hold no authority of their own, and mesh events never run them implicitly."* A content-addressed saved-PTC-program store is a small, clean kernel primitive for prime-agent (item 61).

### AI.3 Harness composition — the shell-first boundary

`docs/harnesses.md`: "compose existing programs, not application-specific tool bridges." Browser/macOS harnesses own their connections, state, and permission checks; fabric supplies only shell + task supervision, passes `--no-auto-allow` so nothing silently approves, and registers **no harness-specific extension**. The principle for prime-agent's growing tool surface: orchestrate existing CLIs through the kernel rather than building per-tool bridges — and never let the orchestrator auto-approve a capability prompt on the tool's behalf.

### AI.4 Component calculus — formal backing, no new action

`docs/component-calculus.md` formalizes the component plane with an effect/coeffect calculus (from DeepSeek's dynamic-composition paper): components as operational tuples over declared `requires`/`provides` refs. No new borrow beyond what pass 8 recorded — the value is knowing the runtime laws are *formally specified*, not ad hoc, if prime-agent ever adopts the component model.

## AJ. pi durable — the design-evolution lessons

### AJ.1 The measured rejection (chord-delta-findings.md)

They **built** an ID-addressed graph tracker for replicated state, **measured** it (20k strokes / 2.04M objects on an M5 Max, 128 GiB), and **rejected** it: "exhaustive reads still allocate gigabytes and became slower" — keeping the simpler tree/path delta implementation, while explicitly noting "this rejects the measured implementation, not every possible graph representation." The lesson for prime-agent's durable direction: **path/tree deltas won on real workloads; graph-shaped state trackers were measured out.** Any future session-state redesign should start from that result, not re-derive it.

### AJ.2 The handoff discipline (pico-v5-handoff.md)

"The spec is normative. Implement this list in order. After every package: run its tests, run check, and stop for user review. **Do not redesign later packages while implementing the current one.**" Packages 1–23 shipped this way; the pico3 prototype kept as reference only — "preserve useful behavior, not its capability facades, membranes, document routing." This is the process pattern for any multi-PR strategic borrow (the roadmap's Phase 5 especially): normative spec, ordered implementation, review gates between packages, no redesigning ahead.

---

## AK. Ranked addition (61)

61. **Saved-program store** — content-addressed PTC/Jev programs by name+digest, authority-free, host-runnable; `<sha256>.json` + index + crash-reaped lock (kernel primitive)

With this pass the design-document layer is exhausted: every fabric doc and every durable design doc has been read at least at head level. The survey stands at 61 items.
