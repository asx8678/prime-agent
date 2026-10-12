//! Prompt-cache warming (TS `core/cache-warmer.ts`): keeps one prompt
//! cache entry alive by re-sending its request with a one-token output cap
//! before the entry expires.
//!
//! Ownership: pa-ai answers the model-level questions (the entry's TTL, and
//! whether a one-token replay is safe — `pa_ai::cache_warming`); this module
//! owns the service: arming on a session request, the refresh schedule
//! (`min(0.9·TTL, TTL−10 s)` with half the pre-expiry margin held back for
//! timer lateness), the warm-or-stop economics (refresh only when the
//! expected saving is at least $0.05), and the one-token replay itself.
//! The host (the daemon worker) supplies the live mode setting, the prompt
//! size of the last real request, the usage-ledger sink for successful
//! refreshes, and the invalidation calls (compaction, model switch, session
//! end, agent-run settle). Warming is fire-and-forget from the turn path:
//! refreshes run on their own spawned task and a hung provider never delays
//! a turn. The host can ask whether a warm window is live
//! ([`CacheWarmer::warming_active`]): the daemon worker defers its own idle
//! passivation while it is, so a short `idleEvictionMinutes` never parks a
//! worker mid-window and wastes the refreshes it already paid for.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use pa_ai::cache_warming::{prompt_cache_ttl_ms, request_is_replayable};
use pa_ai::models::calculate_cost_values;
use pa_ai::types::SimpleStreamOptions;
use pa_types::ai::{Model, StopReason, Usage};
use tokio_util::sync::CancellationToken;

use crate::settings::CacheWarmingMode;

/// Streaming warming never continues past this long after the real request
/// that started it.
const MAX_WARMING_AGE: std::time::Duration = std::time::Duration::from_hours(1);
/// Idle warming uses a shorter horizon because continuation estimates
/// become less reliable with age.
const MAX_IDLE_WARMING_AGE: std::time::Duration = std::time::Duration::from_mins(30);
/// A refresh is sent only when it is expected to save at least this many
/// dollars.
const CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS: f64 = 0.05;
/// Chance that a real request arrives before the cache entry expires while
/// the agent sits idle (measured by pi; per-session estimates were not
/// better than this constant).
const IDLE_CONTINUATION_PROBABILITY: f64 = 0.15;

/// Refresh at 90% of the TTL while preserving at least ten seconds of
/// margin (`getCacheWarmingDelayMs`). A TTL of ten seconds or less has no
/// refresh window at all.
#[must_use]
pub fn cache_warming_delay_ms(ttl_ms: u64) -> Option<u64> {
    if ttl_ms <= 10_000 {
        return None;
    }
    Some(std::cmp::max(1, (ttl_ms * 9 / 10).min(ttl_ms - 10_000)))
}

/// Which horizon an expected-savings estimate uses: `Streaming` while the
/// agent run that sent the request is still active, `Idle` after it settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmingPhase {
    Streaming,
    Idle,
}

/// Pi's warm-or-stop call for one refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheWarmingAction {
    Warm,
    Stop,
}

/// Inputs and outcome of one warm-or-stop decision (`CacheWarmingDecision`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheWarmingDecision {
    pub phase: WarmingPhase,
    /// Price of this refresh: a cache read of the prompt plus one output token.
    pub warm_cost: f64,
    /// Extra price of the next real request if the cache entry is lost.
    pub miss_cost: f64,
    /// Estimated chance that a real request arrives before the entry expires.
    pub continuation_probability: f64,
    /// `continuation_probability * miss_cost - warm_cost`.
    pub expected_savings: f64,
    /// False when the prompt size or the model's prices are unknown.
    pub economics_available: bool,
    /// `Warm` when `expected_savings` is at least $0.05.
    pub action: CacheWarmingAction,
}

/// The price of `tokens` against `model`'s rates (`price`): only the fields
/// set contribute.
fn price(model: &Model, tokens: Usage) -> f64 {
    calculate_cost_values(model, &tokens, None).total.as_f64()
}

/// `price` with only `cache_read` set.
fn cache_read_price(model: &Model, prompt_tokens: u64) -> f64 {
    price(
        model,
        Usage {
            cache_read: prompt_tokens,
            ..Usage::default()
        },
    )
}

/// The warm-or-stop economics for one refresh of `prompt_tokens` on `model`
/// (`CacheWarmer.evaluate`). The miss prices a fresh cache write when the
/// model has cache-write pricing, and a full-price input otherwise.
#[must_use]
pub fn evaluate_warming(
    model: &Model,
    prompt_tokens: u64,
    phase: WarmingPhase,
) -> CacheWarmingDecision {
    let cache_hit_cost = cache_read_price(model, prompt_tokens);
    let miss_tokens = if model.cost.cache_write.as_f64() > 0.0 {
        Usage {
            cache_write: prompt_tokens,
            ..Usage::default()
        }
    } else {
        Usage {
            input: prompt_tokens,
            ..Usage::default()
        }
    };
    let cache_miss_cost = price(model, miss_tokens);
    let warm_cost = cache_read_price(model, prompt_tokens)
        + price(
            model,
            Usage {
                output: 1,
                ..Usage::default()
            },
        );
    let miss_cost = (cache_miss_cost - cache_hit_cost).max(0.0);
    let continuation_probability = match phase {
        WarmingPhase::Streaming => 1.0,
        WarmingPhase::Idle => IDLE_CONTINUATION_PROBABILITY,
    };
    let economics_available = prompt_tokens > 0 && (cache_hit_cost > 0.0 || cache_miss_cost > 0.0);
    let expected_savings = continuation_probability * miss_cost - warm_cost;
    let action = if expected_savings >= CACHE_WARMING_MINIMUM_EXPECTED_SAVINGS {
        CacheWarmingAction::Warm
    } else {
        CacheWarmingAction::Stop
    };
    CacheWarmingDecision {
        phase,
        warm_cost,
        miss_cost,
        continuation_probability,
        expected_savings,
        economics_available,
        action,
    }
}

/// The request whose prompt cache entry should be kept warm, exactly as it
/// was sent (`CacheWarmRequest`).
pub struct WarmRequest {
    pub model: Model,
    pub context: pa_types::ai::Context,
    pub options: SimpleStreamOptions,
}

impl WarmRequest {
    /// The replay's options: the request's own options with a one-token
    /// output cap and a fresh cancellation signal, so a settled turn never
    /// cancels a refresh (TS `refresh`: `maxTokens: 1`, `maxRetries: 0`,
    /// `signal: run.controller.signal`). Pa-ai has no internal retry loop,
    /// so the zero-retry rule holds by construction.
    fn replay_options(&self, signal: CancellationToken) -> SimpleStreamOptions {
        let mut options = self.options.clone();
        options.base.max_tokens = Some(1);
        options.base.signal = Some(signal);
        options
    }
}

/// One successful refresh, ready for the session's usage ledger (TS
/// `appendUsage("cache_warm", ...)` — the host writes the `type: "usage"`
/// session row).
#[derive(Debug, Clone, PartialEq)]
pub struct WarmUsage {
    pub provider: String,
    pub model: String,
    pub usage: Usage,
}

/// The host's usage-ledger sink for successful refreshes. Set after
/// construction (the daemon worker owns the durable session file, so the
/// sink arrives when the worker attaches — the carry-row-sink pattern).
pub type WarmUsageSink = Arc<dyn Fn(WarmUsage) + Send + Sync>;
/// The host's adoption-telemetry note for one successful refresh (the
/// gated `SessionCounters` seam; the count uploads, content never does).
pub type RefreshNote = Arc<dyn Fn() + Send + Sync>;
/// The live warming mode (the host reads its settings; warming spends money,
/// so the setting is global-only).
pub type ModeSource = Arc<dyn Fn() -> CacheWarmingMode + Send + Sync>;

/// The warmer's state for hosts and tests (`CacheWarmingStatus`):
/// `Scheduled` while a refresh timer is armed, `Refreshing` while a warm
/// request is in flight, `Inactive` with the reason nothing runs.
#[derive(Debug, Clone, PartialEq)]
pub enum WarmerState {
    Inactive,
    Scheduled,
    Refreshing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WarmerStatus {
    pub state: WarmerState,
    pub reason: Option<String>,
    pub decision: Option<CacheWarmingDecision>,
}

impl WarmerStatus {
    fn inactive(reason: impl Into<String>) -> Self {
        Self {
            state: WarmerState::Inactive,
            reason: Some(reason.into()),
            decision: None,
        }
    }

    fn running(decision: Option<CacheWarmingDecision>, refreshing: bool) -> Self {
        Self {
            state: if refreshing {
                WarmerState::Refreshing
            } else {
                WarmerState::Scheduled
            },
            reason: None,
            decision,
        }
    }
}

/// Keeps one prompt cache entry alive per session. `start` replaces any
/// previous run; every invalidation (a new request, compaction, a model
/// switch, session end) cancels the in-flight refresh. Refreshes run on a
/// spawned task: nothing on a turn path ever awaits one.
pub struct CacheWarmer {
    inner: Arc<Inner>,
    /// The agent-event subscription feeding this warmer (`MessageEnd`
    /// prompt sizes, `AgentEnd` run settles). Holding the guard here means
    /// dropping the warmer also drops the observer.
    subscription: std::sync::Mutex<Option<pa_agent::agent::Subscription>>,
}

struct Inner {
    mode: ModeSource,
    /// The active run's cancellation token: `start` mints a fresh one
    /// (a `CancellationToken` is single-shot) and every invalidation fires
    /// it, so an in-flight refresh or a pending sleep ends immediately —
    /// the run task must never outlive the session that armed it.
    cancel: std::sync::Mutex<CancellationToken>,
    /// The prompt size of the most recent real request, as reported by the
    /// provider: the last assistant usage's `input + cacheRead +
    /// cacheWrite` (`lastPromptTokens`).
    prompt_tokens: AtomicU64,
    /// The durable usage-ledger sink, attached by the host once its session
    /// store is reachable.
    usage_sink: std::sync::Mutex<Option<WarmUsageSink>>,
    /// Adoption note the host's telemetry counts at each refresh.
    on_refresh: RefreshNote,
    /// The active run's generation; a bumped value means the task must stop.
    generation: AtomicU64,
    /// Set when the agent run settled: the idle horizon replaces the
    /// streaming one and `streaming` mode stops warming.
    settled: AtomicBool,
    /// When the active run's real request went out (the horizon both
    /// phases measure from); `None` before the first arm.
    run_started_at: std::sync::Mutex<Option<tokio::time::Instant>>,
    status: std::sync::Mutex<WarmerStatus>,
}

impl CacheWarmer {
    /// Build a warmer over the host's live mode and refresh note
    /// (`on_refresh` fires once per successful refresh — the host's
    /// telemetry counts it; content never uploads). The prompt size of
    /// the last real request arrives through [`Self::note_prompt_tokens`];
    /// the usage-ledger sink through [`Self::set_usage_sink`].
    #[must_use]
    pub fn new(mode: ModeSource, on_refresh: RefreshNote) -> Self {
        Self {
            inner: Arc::new(Inner {
                mode,
                cancel: std::sync::Mutex::new(CancellationToken::new()),
                prompt_tokens: AtomicU64::new(0),
                usage_sink: std::sync::Mutex::new(None),
                on_refresh,
                generation: AtomicU64::new(0),
                settled: AtomicBool::new(false),
                run_started_at: std::sync::Mutex::new(None),
                status: std::sync::Mutex::new(WarmerStatus::inactive("waiting for first request")),
            }),
            subscription: std::sync::Mutex::new(None),
        }
    }

    /// Adopt the agent-event subscription this warmer's observer holds
    /// (the engine installs it right after building the session).
    ///
    /// # Panics
    ///
    /// Panics when the subscription slot's mutex is poisoned.
    pub fn set_subscription(&self, subscription: pa_agent::agent::Subscription) {
        *self.subscription.lock().expect("warmer subscription lock") = Some(subscription);
    }

    /// Keep the prompt cache entry written by `request` warm while the host
    /// does not invalidate it (`CacheWarmer.start`). Replaces any previous
    /// run — including aborting its in-flight refresh.
    ///
    /// # Panics
    ///
    /// Panics when any warmer mutex is poisoned.
    pub fn start(&self, request: WarmRequest) {
        let generation = self.clear_run();
        let mode = (self.inner.mode)();
        if mode == CacheWarmingMode::Off {
            self.set_status(WarmerStatus::inactive("cache warming disabled"));
            return;
        }
        if !request_is_replayable(&request.model, Some(&request.options)) {
            self.set_status(WarmerStatus::inactive("request cannot be replayed safely"));
            return;
        }
        let Some(ttl_ms) =
            prompt_cache_ttl_ms(&request.model, request.options.base.cache_retention)
        else {
            let reason = if request.options.base.cache_retention
                == Some(pa_types::ai::CacheRetention::None)
            {
                "request disabled prompt caching"
            } else {
                "cache lifetime unavailable"
            };
            self.set_status(WarmerStatus::inactive(reason));
            return;
        };
        let Some(delay_ms) = cache_warming_delay_ms(ttl_ms) else {
            self.set_status(WarmerStatus::inactive("cache lifetime unavailable"));
            return;
        };
        self.inner.settled.store(false, Ordering::Release);
        self.set_status(WarmerStatus::running(None, false));
        let cancel = CancellationToken::new();
        *self.inner.cancel.lock().expect("warmer cancel lock") = cancel.clone();
        let started_at = tokio::time::Instant::now();
        *self
            .inner
            .run_started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(started_at);
        let run = ActiveRun {
            generation,
            request,
            delay_ms,
            ttl_ms,
            started_at,
            cancel,
        };
        tokio::spawn(run_task(Arc::clone(&self.inner), run));
    }

    /// The agent run settled: `streaming` mode stops warming; `idle` mode
    /// continues on the shorter idle horizon (`onAgentSettled`).
    ///
    /// # Panics
    ///
    /// Panics when the status mutex is poisoned.
    pub fn on_agent_settled(&self) {
        let generation = self.inner.generation.load(Ordering::Acquire);
        if generation == 0 {
            return;
        }
        let mode = (self.inner.mode)();
        match mode {
            CacheWarmingMode::Off => {
                self.stop("cache warming disabled");
            }
            CacheWarmingMode::Streaming => {
                self.stop("agent run settled");
            }
            CacheWarmingMode::Idle => {
                self.inner.settled.store(true, Ordering::Release);
                let mut status = self.inner.status.lock().expect("warmer status lock");
                // A run already past the idle horizon stops without another
                // refresh; the loop's own cap check re-verifies at fire time.
                if status.state != WarmerState::Inactive {
                    status.decision = status.decision.map(|decision| CacheWarmingDecision {
                        phase: WarmingPhase::Idle,
                        ..decision
                    });
                }
            }
        }
    }

    /// The conversation context replaced the warmed request's prefix
    /// (compaction, a branch switch, a model change): stop warming it
    /// (`validateRun`'s `isCurrent` stop). A new session request re-arms.
    ///
    /// # Panics
    ///
    /// Panics when the status mutex is poisoned.
    pub fn invalidate(&self, reason: &'static str) {
        self.stop(reason);
    }

    /// Stop warming and record why (`cancel`).
    ///
    /// # Panics
    ///
    /// Panics when the status mutex is poisoned.
    pub fn cancel(&self) {
        self.stop("inactive");
    }

    /// Whether a warm window is live: a run is armed and not stopped
    /// (invalidated, cancelled, settled in `streaming` mode, or ended by the
    /// refresh loop) and still inside its horizon — `MAX_WARMING_AGE` while
    /// the agent run streams, `MAX_IDLE_WARMING_AGE` once it settled, both
    /// measured from the real request exactly as the refresh loop measures
    /// them. Sync and lock-bounded (two brief mutex reads, poison
    /// tolerated): the turn path never awaits it.
    #[must_use]
    pub fn warming_active(&self) -> bool {
        if self.inner.generation.load(Ordering::Acquire) == 0 {
            return false;
        }
        let running = self
            .inner
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
            != WarmerState::Inactive;
        if !running {
            return false;
        }
        let Some(started_at) = *self
            .inner
            .run_started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        else {
            return false;
        };
        let horizon = if self.inner.settled.load(Ordering::Acquire) {
            MAX_IDLE_WARMING_AGE
        } else {
            MAX_WARMING_AGE
        };
        tokio::time::Instant::now() < started_at + horizon
    }

    /// The prompt size of the most recent real request, as reported by the
    /// provider: the last assistant usage's `input + cacheRead +
    /// cacheWrite` (`lastPromptTokens`).
    pub fn note_prompt_tokens(&self, tokens: u64) {
        self.inner.prompt_tokens.store(tokens, Ordering::Release);
    }

    /// The usage-ledger sink for successful refreshes: set once the host's
    /// durable session store is attached; until then refreshes only count.
    ///
    /// # Panics
    ///
    /// Panics when the usage-sink mutex is poisoned.
    pub fn set_usage_sink(&self, sink: WarmUsageSink) {
        *self
            .inner
            .usage_sink
            .lock()
            .expect("warmer usage sink lock") = Some(sink);
    }

    /// A snapshot of the warmer's state for hosts and tests.
    ///
    /// # Panics
    ///
    /// Panics when the status mutex is poisoned.
    #[must_use]
    pub fn status(&self) -> WarmerStatus {
        self.inner
            .status
            .lock()
            .expect("warmer status lock")
            .clone()
    }

    /// Cancel the active run (its in-flight refresh and any pending
    /// sleep) and hand the next one a fresh generation.
    fn clear_run(&self) -> u64 {
        let generation = self.inner.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.inner
            .cancel
            .lock()
            .expect("warmer cancel lock")
            .cancel();
        generation
    }

    fn stop(&self, reason: &'static str) {
        self.clear_run();
        self.set_status(WarmerStatus::inactive(reason));
    }

    fn set_status(&self, status: WarmerStatus) {
        *self.inner.status.lock().expect("warmer status lock") = status;
    }
}

impl Drop for CacheWarmer {
    fn drop(&mut self) {
        // The run task holds `Inner`, not this outer handle, so without
        // this cancel a spawned refresh loop would outlive the session
        // that armed it.
        self.stop("session ended");
    }
}

struct ActiveRun {
    generation: u64,
    request: WarmRequest,
    delay_ms: u64,
    ttl_ms: u64,
    started_at: tokio::time::Instant,
    cancel: CancellationToken,
}

/// The refresh loop for one active run. Every checkpoint re-validates the
/// run: a generation bump (a new request or an invalidation) ends the loop
/// silently — the caller that bumped owns the status now.
async fn run_task(inner: Arc<Inner>, run: ActiveRun) {
    let mut next_warm_at = run.started_at + std::time::Duration::from_millis(run.delay_ms);
    loop {
        let sleep = tokio::time::sleep_until(next_warm_at);
        tokio::select! {
            () = run.cancel.cancelled() => return,
            () = sleep => {}
        }
        if inner.generation.load(Ordering::Acquire) != run.generation {
            return;
        }
        let now = tokio::time::Instant::now();
        // A late refresh is likely a full-price cache write, not a cache
        // warm: keep half the planned pre-expiry margin for timer lateness
        // and request dispatch (`schedule`).
        let refresh_deadline_at =
            next_warm_at + std::time::Duration::from_millis((run.ttl_ms - run.delay_ms) / 2);
        if now > refresh_deadline_at {
            inner.stop_stale(run.generation, "cache refresh deadline missed");
            return;
        }
        let mode = (inner.mode)();
        let phase = if inner.settled.load(Ordering::Acquire) {
            WarmingPhase::Idle
        } else {
            WarmingPhase::Streaming
        };
        if mode == CacheWarmingMode::Off {
            inner.stop_stale(run.generation, "cache warming disabled");
            return;
        }
        if mode == CacheWarmingMode::Streaming && phase == WarmingPhase::Idle {
            inner.stop_stale(run.generation, "agent run settled");
            return;
        }
        let horizon = match phase {
            WarmingPhase::Streaming => MAX_WARMING_AGE,
            WarmingPhase::Idle => MAX_IDLE_WARMING_AGE,
        };
        if now >= run.started_at + horizon {
            let reason = match phase {
                WarmingPhase::Idle => "30-minute idle safety limit reached",
                WarmingPhase::Streaming => "one-hour safety limit reached",
            };
            inner.stop_stale(run.generation, reason);
            return;
        }
        let prompt_tokens = inner.prompt_tokens.load(Ordering::Acquire);
        let decision = evaluate_warming(&run.request.model, prompt_tokens, phase);
        if decision.action == CacheWarmingAction::Stop {
            let reason = if decision.economics_available {
                "expected savings below threshold"
            } else {
                "cache economics unavailable"
            };
            inner.stop_stale(run.generation, reason);
            return;
        }
        inner.set_running(run.generation, Some(decision), true);
        // The replay: the request exactly as sent, one output token, on a
        // child of the run's cancel token so `start`/`invalidate` can abort
        // an in-flight refresh. Warming is best-effort: a failed stream
        // only ends this run's reschedule decision below.
        let replay_signal = run.cancel.child_token();
        let replay = run.request.replay_options(replay_signal);
        let message =
            match pa_ai::stream_simple(&run.request.model, &run.request.context, Some(replay)) {
                Ok(stream) => Some(stream.result().await),
                Err(_) => None,
            };
        if inner.generation.load(Ordering::Acquire) != run.generation {
            return;
        }
        if let Some(message) = message {
            if !matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
                let usage = WarmUsage {
                    provider: message.provider.clone(),
                    model: message
                        .response_model
                        .unwrap_or_else(|| message.model.clone()),
                    usage: message.usage,
                };
                if let Some(sink) = inner
                    .usage_sink
                    .lock()
                    .expect("warmer usage sink lock")
                    .as_ref()
                {
                    sink(usage);
                }
                (inner.on_refresh)();
            }
        }
        next_warm_at = tokio::time::Instant::now() + std::time::Duration::from_millis(run.delay_ms);
        inner.set_running(run.generation, None, false);
    }
}

impl Inner {
    /// Stop the run only when it is still the active one: the generation
    /// re-check under the status lock closes the race with a concurrent
    /// `start` (which bumps the generation before it takes the lock), so a
    /// stale stop can never clobber the new run's status.
    fn stop_stale(&self, generation: u64, reason: &str) {
        let mut status = self.status.lock().expect("warmer status lock");
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        *status = WarmerStatus::inactive(reason);
    }

    fn set_running(
        &self,
        generation: u64,
        decision: Option<CacheWarmingDecision>,
        refreshing: bool,
    ) {
        let mut status = self.status.lock().expect("warmer status lock");
        if self.generation.load(Ordering::Acquire) != generation {
            return;
        }
        *status = WarmerStatus::running(decision, refreshing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt as _;
    use tokio::io::AsyncWriteExt as _;

    fn priced_model(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Model {
        serde_json::from_value(serde_json::json!({
            "id": "glm-test", "name": "GLM test", "api": "openai-completions",
            "provider": "prime-inference", "baseUrl": "https://example.invalid",
            "reasoning": false, "input": ["text"],
            "cost": {"input": input, "output": output, "cacheRead": cache_read, "cacheWrite": cache_write},
            "contextWindow": 131_072, "maxTokens": 8192,
        }))
        .expect("model")
    }

    #[test]
    fn the_refresh_delay_refreshes_at_90_percent_with_a_ten_second_floor() {
        assert_eq!(cache_warming_delay_ms(10_000), None);
        assert_eq!(cache_warming_delay_ms(9_999), None);
        assert_eq!(
            cache_warming_delay_ms(10_001),
            Some(1),
            "the floor dominates"
        );
        assert_eq!(cache_warming_delay_ms(20_000), Some(10_000));
        assert_eq!(
            cache_warming_delay_ms(100_000),
            Some(90_000),
            "the crossover"
        );
        assert_eq!(cache_warming_delay_ms(300_000), Some(270_000));
    }

    #[test]
    fn streaming_economics_of_a_full_context_prefix_warm() {
        // $/million rates chosen so every cost is float-exact.
        let model = priced_model(300.0, 1_000_000.0, 30.0, 375.0);
        let decision = evaluate_warming(&model, 1_000_000, WarmingPhase::Streaming);
        assert_eq!(
            decision,
            CacheWarmingDecision {
                phase: WarmingPhase::Streaming,
                warm_cost: 31.0,
                miss_cost: 345.0,
                continuation_probability: 1.0,
                expected_savings: 314.0,
                economics_available: true,
                action: CacheWarmingAction::Warm,
            }
        );
    }

    #[test]
    fn idle_economics_discount_the_miss_by_the_continuation_probability() {
        let model = priced_model(300.0, 1_000_000.0, 30.0, 375.0);
        let decision = evaluate_warming(&model, 1_000_000, WarmingPhase::Idle);
        let expected = 0.15 * 345.0 - 31.0;
        assert_eq!(decision.continuation_probability, 0.15);
        assert_eq!(decision.expected_savings, expected);
        assert_eq!(decision.action, CacheWarmingAction::Warm);
    }

    #[test]
    fn a_worthless_entry_never_refreshes() {
        // cache-read equals cache-write: keeping the entry saves nothing, so
        // the refresh itself is a pure loss.
        let model = priced_model(300.0, 1_000_000.0, 1_000.0, 1_000.0);
        let decision = evaluate_warming(&model, 1_000_000, WarmingPhase::Streaming);
        assert_eq!(decision.miss_cost, 0.0);
        assert_eq!(decision.expected_savings, -1_001.0);
        assert!(decision.economics_available);
        assert_eq!(decision.action, CacheWarmingAction::Stop);
    }

    #[test]
    fn a_model_without_cache_write_pricing_misses_at_the_input_rate() {
        let model = priced_model(4_000.0, 1_000_000.0, 1_000.0, 0.0);
        let decision = evaluate_warming(&model, 1_000_000, WarmingPhase::Streaming);
        assert_eq!(
            decision.miss_cost, 3_000.0,
            "input-priced, not write-priced"
        );
        assert_eq!(decision.expected_savings, 1_999.0);
        assert_eq!(decision.action, CacheWarmingAction::Warm);
    }

    #[test]
    fn unknown_economics_stop_the_run() {
        let model = priced_model(300.0, 15.0, 30.0, 375.0);
        let decision = evaluate_warming(&model, 0, WarmingPhase::Streaming);
        assert!(!decision.economics_available);
        assert_eq!(decision.action, CacheWarmingAction::Stop);
    }

    // ---- The service, against scripted providers on a local listener ----

    struct Harness {
        warmer: CacheWarmer,
        mode: Arc<std::sync::Mutex<CacheWarmingMode>>,
        usage: Arc<std::sync::Mutex<Vec<WarmUsage>>>,
        refreshes: Arc<AtomicU64>,
    }

    fn harness() -> Harness {
        let mode = Arc::new(std::sync::Mutex::new(CacheWarmingMode::Idle));
        let usage = Arc::new(std::sync::Mutex::new(Vec::new()));
        let refreshes = Arc::new(AtomicU64::new(0));
        let mode_for_warmer = Arc::clone(&mode);
        let usage_for_warmer = Arc::clone(&usage);
        let refreshes_for_warmer = Arc::clone(&refreshes);
        let usage_sink: WarmUsageSink =
            Arc::new(move |entry| usage_for_warmer.lock().unwrap().push(entry));
        let warmer = CacheWarmer::new(
            Arc::new(move || *mode_for_warmer.lock().unwrap()),
            Arc::new(move || {
                refreshes_for_warmer.fetch_add(1, Ordering::AcqRel);
            }),
        );
        warmer.set_usage_sink(usage_sink);
        warmer.note_prompt_tokens(1_000_000);
        Harness {
            warmer,
            mode,
            usage,
            refreshes,
        }
    }

    fn warm_request(addr: std::net::SocketAddr) -> WarmRequest {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "glm-test", "name": "GLM test", "api": "openai-completions",
            "provider": "prime-inference", "baseUrl": format!("http://{addr}"),
            "reasoning": false, "input": ["text"],
            "cost": {"input": 300, "output": 15, "cacheRead": 30, "cacheWrite": 375},
            "contextWindow": 131_072, "maxTokens": 8192,
            "promptCache": {"short": 11}
        }))
        .expect("model");
        let message: pa_types::ai::Message = serde_json::from_value(serde_json::json!({
            "role": "user", "content": [{"type": "text", "text": "hi"}],
            "timestamp": 0
        }))
        .expect("message");
        WarmRequest {
            model,
            context: pa_types::ai::Context {
                system_prompt: Some("warm".to_string()),
                messages: vec![message],
                tools: None,
            },
            options: SimpleStreamOptions {
                base: pa_ai::types::StreamOptions {
                    api_key: Some("test".to_string()),
                    session_id: Some("session-1".to_string()),
                    ..Default::default()
                },
                reasoning: None,
                thinking_budgets: None,
            },
        }
    }

    /// A listener answering every request with a complete SSE exchange;
    /// `requests` counts accepted connections.
    async fn scripted_provider(requests: Arc<AtomicU64>) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                requests.fetch_add(1, Ordering::AcqRel);
                let mut request = vec![0u8; 8192];
                let _ = socket.read(&mut request).await;
                let body = "data: {\"id\":\"w1\",\"object\":\"chat.completion.chunk\",\"model\":\"glm-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\".\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        addr
    }

    async fn wait_until(what: &str, mut probe: impl FnMut() -> bool) {
        for _ in 0..100 {
            if probe() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn a_refresh_replays_the_request_and_reschedules() {
        let harness = harness();
        let requests = Arc::new(AtomicU64::new(0));
        let addr = scripted_provider(Arc::clone(&requests)).await;
        harness.warmer.start(warm_request(addr));
        // The TTL is 11 s: the first refresh is due after 1 s.
        wait_until("two refreshes", || requests.load(Ordering::Acquire) >= 2).await;
        let usage = harness.usage.lock().unwrap();
        let expected_entry = WarmUsage {
            provider: "prime-inference".to_string(),
            model: "glm-test".to_string(),
            usage: Usage::default(),
        };
        assert_eq!(
            usage.as_slice(),
            vec![expected_entry.clone(), expected_entry].as_slice()
        );
        assert_eq!(harness.refreshes.load(Ordering::Acquire), 2);
        assert_eq!(harness.warmer.status().state, WarmerState::Scheduled);
        harness.warmer.cancel();
    }

    #[tokio::test]
    async fn streaming_mode_stops_at_settle_and_idle_mode_continues() {
        let streaming = harness();
        *streaming.mode.lock().unwrap() = CacheWarmingMode::Streaming;
        let requests = Arc::new(AtomicU64::new(0));
        let addr = scripted_provider(Arc::clone(&requests)).await;
        streaming.warmer.start(warm_request(addr));
        wait_until("one refresh", || requests.load(Ordering::Acquire) >= 1).await;
        streaming.warmer.on_agent_settled();
        let status = streaming.warmer.status();
        assert_eq!(status.state, WarmerState::Inactive);
        assert_eq!(status.reason.as_deref(), Some("agent run settled"));
        let settled_at = requests.load(Ordering::Acquire);
        tokio::time::sleep(std::time::Duration::from_millis(1_400)).await;
        assert_eq!(
            requests.load(Ordering::Acquire),
            settled_at,
            "streaming mode sends no refresh after the run settled"
        );

        // Idle mode: settling keeps warming on the idle horizon.
        let idle = harness();
        let idle_requests = Arc::new(AtomicU64::new(0));
        let idle_addr = scripted_provider(Arc::clone(&idle_requests)).await;
        idle.warmer.start(warm_request(idle_addr));
        wait_until("one refresh", || idle_requests.load(Ordering::Acquire) >= 1).await;
        idle.warmer.on_agent_settled();
        wait_until("a post-settle refresh", || {
            idle_requests.load(Ordering::Acquire) >= 2
        })
        .await;
    }

    /// `warming_active` across the warmer's states: unarmed, armed while
    /// streaming, stopped at a streaming-mode settle, idle mode after a
    /// settle, and stopped by `invalidate` or `cancel`.
    #[tokio::test]
    async fn warming_active_follows_the_warm_window() {
        let requests = Arc::new(AtomicU64::new(0));
        let addr = scripted_provider(Arc::clone(&requests)).await;

        let streaming = harness();
        *streaming.mode.lock().unwrap() = CacheWarmingMode::Streaming;
        assert!(!streaming.warmer.warming_active(), "unarmed");
        streaming.warmer.start(warm_request(addr));
        assert!(streaming.warmer.warming_active(), "armed while streaming");
        streaming.warmer.on_agent_settled();
        assert!(
            !streaming.warmer.warming_active(),
            "streaming mode stops at the settle"
        );

        let idle = harness();
        idle.warmer.start(warm_request(addr));
        idle.warmer.on_agent_settled();
        assert!(
            idle.warmer.warming_active(),
            "idle mode warms after the settle"
        );
        idle.warmer.invalidate("conversation context changed");
        assert!(!idle.warmer.warming_active(), "invalidated");

        let cancelled = harness();
        cancelled.warmer.start(warm_request(addr));
        cancelled.warmer.cancel();
        assert!(!cancelled.warmer.warming_active(), "cancelled");

        // A request the warmer refuses never opens a window.
        let off = harness();
        *off.mode.lock().unwrap() = CacheWarmingMode::Off;
        off.warmer.start(warm_request(addr));
        assert!(!off.warmer.warming_active(), "mode off");
    }

    /// The horizon bounds the window even between refresh-loop wakeups: an
    /// idle window ends 30 minutes after the real request, a streaming one
    /// is still live then. A one-hour TTL keeps the refresh loop asleep
    /// past both checks, so the predicate's own horizon decides.
    #[tokio::test(start_paused = true)]
    async fn warming_active_ends_at_the_horizon() {
        let requests = Arc::new(AtomicU64::new(0));
        let addr = scripted_provider(Arc::clone(&requests)).await;
        let long_lived = || {
            let mut request = warm_request(addr);
            request.model.prompt_cache = Some(pa_types::ai::ModelPromptCache {
                short: Some(3_600),
                long: None,
            });
            request
        };
        let streaming = harness();
        *streaming.mode.lock().unwrap() = CacheWarmingMode::Streaming;
        streaming.warmer.start(long_lived());
        let idle = harness();
        idle.warmer.start(long_lived());
        idle.warmer.on_agent_settled();
        tokio::time::advance(MAX_IDLE_WARMING_AGE + std::time::Duration::from_secs(1)).await;
        assert!(
            streaming.warmer.warming_active(),
            "a streaming window runs to the one-hour horizon"
        );
        assert!(
            !idle.warmer.warming_active(),
            "an idle window ends at the 30-minute horizon"
        );
        assert_eq!(
            requests.load(Ordering::Acquire),
            0,
            "no refresh was due yet"
        );
    }

    #[tokio::test]
    async fn invalidation_cancels_the_scheduled_refresh() {
        let harness = harness();
        let requests = Arc::new(AtomicU64::new(0));
        let addr = scripted_provider(Arc::clone(&requests)).await;
        harness.warmer.start(warm_request(addr));
        harness.warmer.invalidate("conversation context changed");
        let status = harness.warmer.status();
        assert_eq!(status.state, WarmerState::Inactive);
        assert_eq!(
            status.reason.as_deref(),
            Some("conversation context changed")
        );
        tokio::time::sleep(std::time::Duration::from_millis(1_400)).await;
        assert_eq!(requests.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn a_disabled_mode_or_unsafe_or_unknown_request_never_arms() {
        let harness = harness();
        *harness.mode.lock().unwrap() = CacheWarmingMode::Off;
        let requests = Arc::new(AtomicU64::new(0));
        let addr = scripted_provider(Arc::clone(&requests)).await;
        harness.warmer.start(warm_request(addr));
        assert_eq!(
            harness.warmer.status().reason.as_deref(),
            Some("cache warming disabled")
        );

        *harness.mode.lock().unwrap() = CacheWarmingMode::Idle;
        // Budget-based Anthropic reasoning: the replay would change the
        // thinking budget the cache entry is keyed on.
        let mut unsafe_request = warm_request(addr);
        unsafe_request.model.api =
            serde_json::from_value(serde_json::json!("anthropic-messages")).expect("api");
        unsafe_request.model.id = "claude-opus-4-5".to_string();
        unsafe_request.options.reasoning = Some(pa_types::ai::ModelThinkingLevel::High);
        harness.warmer.start(unsafe_request);
        assert_eq!(
            harness.warmer.status().reason.as_deref(),
            Some("request cannot be replayed safely")
        );

        // No prompt-cache lifetime on the model: the entry is never warmed.
        let mut unknown_ttl = warm_request(addr);
        unknown_ttl.model.prompt_cache = None;
        harness.warmer.start(unknown_ttl);
        assert_eq!(
            harness.warmer.status().reason.as_deref(),
            Some("cache lifetime unavailable")
        );
        tokio::time::sleep(std::time::Duration::from_millis(1_400)).await;
        assert_eq!(requests.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn unavailable_economics_stop_before_the_first_request() {
        let harness = harness();
        harness.warmer.note_prompt_tokens(0);
        let requests = Arc::new(AtomicU64::new(0));
        let addr = scripted_provider(Arc::clone(&requests)).await;
        harness.warmer.start(warm_request(addr));
        wait_until("the economics stop", || {
            harness.warmer.status().state == WarmerState::Inactive
        })
        .await;
        assert_eq!(
            harness.warmer.status().reason.as_deref(),
            Some("cache economics unavailable")
        );
        assert_eq!(requests.load(Ordering::Acquire), 0);
    }

    /// The paint-latency rule, red-first: a provider that never answers must
    /// not wedge warming — a new session request re-arms, cancels the hung
    /// refresh, and its own refresh lands. Nothing on a turn path ever
    /// awaits a refresh.
    #[tokio::test]
    async fn a_hanging_provider_never_blocks_re_arm_or_refresh() {
        let harness = harness();
        // The hanging provider: accepts, reads the request, never answers.
        let hanging = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hanging_addr = hanging.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((mut socket, _)) = hanging.accept().await {
                let mut request = vec![0u8; 8192];
                let _ = socket.read(&mut request).await;
                // Hold the socket open and never answer: a dropped socket
                // would answer the read with EOF instead of hanging it.
                held.push(socket);
            }
        });
        let armed_at = std::time::Instant::now();
        harness.warmer.start(warm_request(hanging_addr));
        assert!(
            armed_at.elapsed() < std::time::Duration::from_millis(100),
            "arming never awaits a refresh"
        );
        // Wait past the refresh deadline so the hung refresh is in flight.
        tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
        assert_eq!(harness.warmer.status().state, WarmerState::Refreshing);

        // The next session request re-arms while the refresh hangs.
        let healthy_requests = Arc::new(AtomicU64::new(0));
        let healthy_addr = scripted_provider(Arc::clone(&healthy_requests)).await;
        let re_armed_at = std::time::Instant::now();
        harness.warmer.start(warm_request(healthy_addr));
        assert!(
            re_armed_at.elapsed() < std::time::Duration::from_millis(100),
            "re-arming never awaits the hung refresh"
        );
        wait_until("the healthy refresh lands", || {
            harness.refreshes.load(Ordering::Acquire) >= 1
        })
        .await;
        assert_eq!(healthy_requests.load(Ordering::Acquire), 1);
    }
}
