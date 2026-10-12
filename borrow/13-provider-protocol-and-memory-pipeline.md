# Pass 13: the provider protocol, the memory pipeline, and the pinned runner

Thirteenth pass, 2026-10-11 — the last unread high-value docs: fabric's external-provider protocol and durable-pi runner, codex's memories pipeline (grounding item 3.8), TS's remote catalog client, and the plugin sourcing model. Plus the roadmap v2 refresh in [09](09-pockets-and-roadmap.md). Nothing here is implemented.

---

## AF. pi-fabric

### AF.1 The external-provider protocol

`docs/providers.md`: normal `pi.registerTool()` tools are captured automatically; non-tool capabilities use a **versioned provider protocol** — `FABRIC_PROVIDER_REGISTER/DISCOVER` events, `FabricProvider {name, description, list(), describe()}` — mounted either as pinned first-party components or through supervised external components with **staged publication and rolling replacement** (`context.provide()`), the component link also controlling dependency withdrawal. Third parties can therefore ship *action catalogs with risk data*, not just tools. For prime-agent: the reference shape if packages/extensions ever expose provider-style surfaces beyond tools.

### AF.2 The durable-pi runner — pinned-SDK isolation

`docs/durable-pi.md`: `agents.runner` defaults to **`pi-durable`** — an isolated Pi SDK subprocess with a real `AgentSession`, native RPC, and a `DurableAgent` on the published durable Harness; the worker maps Pi imports to a **pinned SDK installed under an npm alias** (`pi-fabric-worker-sdk`) so child module resolution never borrows Main's host SDK; registration and idle hooks load nothing; fallback `runner: "pi"`. The transferable pattern for prime-agent's RLM children: **children run in pinned, version-isolated runtimes** — dependencies declared and shipped with the orchestrator, not inherited from the host process.

## AG. codex — the memories pipeline (grounds item 3.8)

`codex-rs/memories/`: a phased write pipeline — `memories/write` owns Phase 1/2 prompt rendering, filesystem artifact helpers, **workspace diff helpers**, and extension resource pruning — and `memories/read` owns **developer-instruction injection**, **memory citation parsing**, and **read-usage telemetry classification**, with prompt templates beside their crates. Memories are injected as developer instructions, *cited* by the model, and their reads are measured. This is the concrete shape for prime-agent's Continual-Harness memory endgame: injection point, citation grammar, usage telemetry, and a write pipeline that diffs the workspace before persisting — so a memory reflects verified change, not aspiration.

## AH. pi — catalog client and plugin sourcing

- **`remote-catalog-provider.ts`**: the catalog client fetches from `https://pi.dev` with a 4 s attempt timeout, a **4-hour refresh interval**, and a `?types=` negotiation for full-type shards (servers that ignore it still serve the chat-only shard — backward compatible). pa-models has the live catalog and fixtures; the timeout/refresh/type-shard parameters are the operational details worth mirroring.
- **`experimental/plugins/{bundled,package}.ts` + `services/`**: plugins come from **bundled or package-sourced** origins; services compose an agent-controller, models-provider, and presentation-ui over the connection layer. The plugin *sourcing* model (bundled + package) is the detail the extension-seam decision (item 18) will need.

---

**Roadmap v2 is in [09](09-pockets-and-roadmap.md)**: items 57–60 placed, Phase 1 measured at < 500 LOC, corrections applied, the Python-fabric lane recorded as item 18's on-ramp, and item 3.8 grounded. The folder is again internally consistent end to end.
