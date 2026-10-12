# Task: defer a worker's idle passivation while its cache warm window is live

You are implementing one PR in the Prime Agent Rust repo (`PrimeIntellect-ai/prime-agent`, branch `main`). Read `AGENTS.md` first — its merge gates and style rules apply. One logical change per PR; no new dependencies; no commits without the ambient git identity (never set user.name/email).

## Background (verified — do not re-derive)

Prime Agent keeps provider prompt caches alive with a cache warmer (the TS `CacheWarmer` port). Sessions hosted by the daemon get one: the daemon's `build_session` (`crates/pa-daemon/src/agent_engine/lifecycle.rs:890-1033`) passes `cache_warm_arm: Some(warm_arm)` for **every** session — subagents included — and the engine builds the warmer whenever that slot is present (`crates/pa-core/src/session_engine/engine.rs:~860-905`; there is **no depth gate on warming**). Under `cacheWarming: "idle"` a session's cache keeps refreshing for up to `MAX_IDLE_WARMING_AGE` (30 min, `crates/pa-core/src/session_engine/cache_warmer.rs:28-30`) after its run settles.

Idle workers self-passivate: the worker process decides it has been idle past `idleEvictionMinutes` (settings key; `get_idle_eviction()` in `crates/pa-core/src/settings/manager.rs:796`; default 45; `"off"` supported) and sends the `worker_idle_passivation` request (wire name: `worker_idle_passivation`; command: `DaemonCommand::WorkerIdlePassivation` — see `crates/pa-daemon/src/protocol.rs:919,942`, the request-composition site around `crates/pa-daemon/src/worker/turn.rs:291`, and the supervisor handler `handle_worker_idle_passivation` in `crates/pa-daemon/src/supervisor/worker_lifecycle.rs:587-668`). Passivation stops the worker process.

**The bug this PR fixes:** when `idleEvictionMinutes` is configured **below** the active warm window (e.g. 10 < 30), the worker parks itself mid-window. The warmer task dies with the process; every refresh it already paid for is wasted, and the session's cache is cold on wake. Under default settings (45 > 30) the path never fires — this is an edge hardening that must not change default behavior.

## What to build

1. **A public predicate on `CacheWarmer`** (`crates/pa-core/src/session_engine/cache_warmer.rs`): `pub fn warming_active(&self) -> bool` — true when a warm window is live: armed (generation > 0), not invalidated or cancelled, and inside the active horizon (streaming mode: within `MAX_WARMING_AGE` of the real request; idle mode: within `MAX_IDLE_WARMING_AGE` of settle). The read must hold the status lock only briefly and must not panic on poisoning (either tolerate it or document `# Panics` — the workspace enforces `missing_panics_doc`/`missing_errors_doc` via clippy `-D warnings`).
2. **Expose the warmer's state from the engine**: `SessionEngine` (`crates/pa-core/src/session_engine/engine.rs`) gains a field holding the built warmer (e.g. `cache_warmer: Option<Arc<cache_warmer::CacheWarmer>>`) plus an accessor; set it in the existing `if let Some(warm_arm)` build block. `None` keeps headless embeddings untouched.
3. **Defer the worker's own idle passivation while warming**: at the worker-side site that decides to send `worker_idle_passivation` (trace the thread: `worker/turn.rs:291` composes the request; find the idle timer that triggers it), consult the engine's `warming_active()`. While true, **do not send the request this cycle** — reschedule the idle check instead (the deferral must be re-evaluated, not sticky). This is a worker-process-local decision; no cross-process fence, no supervisor-side changes, no new wire surface.
4. **Observability**: log the deferral once per transition (entering deferral), not per timer tick — tracing at the existing passivation log site's level. No new telemetry event (not a user-visible feature surface; state this reasoning in the PR body).

## Hard constraints

- Do not touch the warmer's economics: the $0.05 expected-saving gate, the 0.15 continuation probability, the refresh schedule, or `on_agent_settled`'s mode semantics stay exactly as they are.
- Do not change default behavior: mode `off`, streaming-after-settle (already stopped), expired windows, unarmed sessions, and default `idleEvictionMinutes` must all behave identically to today.
- Do not alter the supervisor's passivation semantics (client-owned refusals, lease handling) or rename any wire command.
- The turn path must never await warming state — the predicate is sync and lock-bounded (the existing fire-and-forget red-first test for the warmer must keep passing).
- **Parity check before merge**: the TS checkout at `~/prime-agent` (read-only) is ground truth. Check whether its `CacheWarmer`/`idleEvictionMinutes` interplay includes an equivalent deferral. If TS lacks it, this is a deliberate Rust-side hardening: flag it in the PR body as pending TS sign-off (the same pattern as `next-steps/07-rlm-api-trio-decision.md`), and do not present it as parity work.

## Tests (repo rules: observable readiness, no fixed sleeps, a regression test that fails without the fix)

- **pa-core unit**: a truth table for `warming_active()` — unarmed; armed + streaming within window; streaming after `on_agent_settled` (stopped → false); armed + idle mode within window; idle past `MAX_IDLE_WARMING_AGE` (false); after `invalidate(...)`; after `cancel()`. Extend the existing `CacheWarmer` tests in `cache_warmer.rs` — do not build a parallel harness.
- **pa-daemon worker**: a worker whose warmer is in an active idle window with `idleEvictionMinutes` set below the window sends **no** `worker_idle_passivation` while warm, and sends it after the window invalidates/expires. Red-first: without the deferral this test must fail. Follow the existing idle-passivation test patterns (`crates/pa-daemon/src/supervisor/tests.rs:1133-1217`).

## Docs

- New fragment `.changes/warm-worker-passivation-deferral.md` — one bullet, plain language ("A worker whose cache is still being kept warm no longer parks itself mid-window when `idleEvictionMinutes` is set shorter than the warm window; the passivation resumes once the window ends.").
- Update the module doc in `cache_warmer.rs` (the ownership paragraph) and the worker idle-passivation doc comment to mention the deferral.

## Definition of done

- [ ] `warming_active()` predicate + engine accessor + worker-side deferral implemented
- [ ] Both test groups pass, and the daemon test is red without the deferral
- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean (doc lints enforced)
- [ ] `cargo test --workspace` green (known environmental exception: `packaged_layout_e2e` needs `objdump` on PATH)
- [ ] Fragment + doc comments written; PR body states the parity finding and the no-new-telemetry reasoning
