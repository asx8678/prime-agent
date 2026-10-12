# Pass 17: the exotic core — Bend, entropy, the actor mesh

Seventeenth pass, 2026-10-11 — the layers only skimmed before: the Bend verified-kernel language (laws, proofs, compiled ABIs), the full entropy system (metric v3, the compiler, review signals), and the actor mesh (event store, delivery policies, validity predicates, effect conflicts). Extends [02](02-pi-and-fabric-deep-dive.md)–[16](16-backlog-verification.md). Nothing here is implemented.

---

## AO. Bend — laws as kernel↔spec correspondences

`LAWS.bend` + `proofs/`: laws are **universally quantified equalities between the kernel and its specification**, stated in both directions. Example set (authority domain):

- `authority_issue_exact`: the kernel's `authorityIssue(grants)` *equals* the spec state constructor — implementation and spec are the same object;
- `authority_use_safe` + `authority_use_complete`: `kernelAllows(authority, grant)` ⟺ `spec.CanUse(authority, grant)` — the correspondence is proven **safe and complete**, not merely sound;
- `authority_derive_safe`: derivation cannot escape the spec's safety predicate.

The pipeline per domain (resource, state, provider, authority, lifecycle, storage): `spec.bend` + `kernel.bend` + `plans.bend` + `proof.bend` → compiled `abi.json` → generated `types.d.ts` — production imports the compiled kernel, and (per `verified-kernels.md`) there is **no handwritten fallback**. The `verified/` modules are that spine: mesh storage revisions, effect-conflict policy, dispatch plans, and authority all call compiled-proved code at runtime.

**The borrow is not the Bend toolchain** — it is the *law shape*: invariants stated as bidirectional kernel↔spec correspondence, with spec-independent predicates and an acceptance ledger (pass 14). Prime-agent's structural-compaction invariants (`fold(state+events) ≡ fold(all)`, the byte bounds, no-drift) are exactly the kind of properties this shape pins — in Rust, as paired property tests asserting both directions (item 64).

## AP. Entropy — the anti-gaming design, fully read

Metric v3 (`docs/entropy.md` §Metric): `score = invocationRejections / actionOperations`, where a rejection is a failure at **resolve, prepare, or validate** (the invocation stages — the model failing to call correctly); discovery and workflow ops are excluded from the denominator; other failures, successes, aborts, and timeouts stay in. Crucially: **it is not a task-success metric** — an all-effect-failed corpus scores *zero*. Idempotent over repeats; per-ref, per-model, and per-1k views.

The anti-gaming guards are explicit: shape entropy, failure-stage entropy, retry churn, navigation, flow, repair lexicon, and schema freedom are **diagnostics only**; `staticScore` (0.25 × call-weighted schema freedom) is *not added*; and — the load-bearing sentence — **"diverse valid calls are not a defect, and removing an enum member cannot improve this objective."** Capability-shrinking is structurally unable to game the metric. Evidence rules match: typed traces only ("never rendered prose"), verbatim audits take precedence over projections, schema digests bind plans to declarations, and catalog repair rows "do not authorize schema restrictions."

The compiler (`src/entropy/`): only **normal-form** proposals auto-apply (provably surface-preserving); the interesting output is the **review-signal taxonomy** — `declare-enum` (a parameter that only ever takes N values → should be an enum), `overload-split` (key-set clusters → should be overloads), `sequence-fuse` (a call sequence that always runs fused → should be one operation) — **friction-derived API redesign proposals for humans**, bounded by `MAX_*` caps, shaped from execution traces and session JSONL.

**For prime-agent (item 63):** the borrowable core of item 53 is this design, not the code: a rejection-rate metric that cannot be gamed by shrinking tools, plus the three review signals as the taxonomy for kernel-API ergonomics review.

## AQ. The actor mesh

- **The event spine** (`mesh/store.ts`): append-only, per-topic **sequenced** events (`MeshEvent {id, sequence, topic, kind, from, to?}`) with owner-identity liveness locks, paged reads, and scoped external grants that stamp events `untrusted` — all landing on the **verified storage kernel** (`storageRevision`). The mesh is the replication backbone the state layer and decision store build on.
- **Typed delivery policies** (`actors/delivery-policy.ts`): **active** deliveries (`steer`, `followUp` — interrupt the current turn) require an explicit `triggerTurn` boolean; **passive** (`mailbox`, `nextTurn`) don't. Invalid combinations throw at construction. Steering-from-children has typed, validated semantics.
- **Validity predicates as bounded programs** (`actors/predicate.ts`): an actor's "valid-while" condition is a **QuickJS program** — source ≤ 16,000 chars, **100 ms timeout, 16 MB memory**, lazily-loaded runtime. Actor liveness is a *declarative, sandboxed, resource-bounded condition*, not a polling loop.
- **Effect conflicts through the verified kernel** (`components/effect-policy.ts`): labeled registrations + bounded resource groups + `knownConflict`/`unknownConflict` from `verified/policy` — with `FabricComponentIndependenceError` for independence violations. Item 24's "declared blast radius" is concretely this, executed by proved code.
- **Jev serving** (`jev-fabric/serve.ts`): a subprocess with a banner handshake (protocol number, version, features), 15 s banner timeout, 16 MiB line caps, 5 s close grace — the judgment engine runs out-of-process over a bounded JSONL protocol.

**For prime-agent:** the validity-predicate pattern (item 62) is immediately useful for RLM children and foreman-style watchers — liveness conditions as bounded declarative programs rather than ad-hoc polling; typed delivery semantics map onto steering queues; the mesh spine remains Phase-5 material.

---

## AR. Ranked additions (62–64)

62. **Actor validity predicates** — bounded QuickJS liveness conditions for children/watchers (pa-daemon, pairs with 15/50)
63. **Entropy v3 metric + review-signal taxonomy** — the anti-gamable ergonomics core: declare-enum / overload-split / sequence-fuse (refines item 53)
64. **Law shape as property tests** — kernel↔spec correspondence pairs, safe *and* complete, for the structural-compaction invariants (refines pass 14 §G.2)

Sixty-four items. The exotic core turned out to be the most precisely engineered part of fabric — and its borrowable essence is shapes, not systems: bidirectional law statements, ungameable metrics, and bounded declarative liveness.
