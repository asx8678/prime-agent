//! Cache-miss attribution: why a request's prompt-cache read dropped
//! versus the previous request. The classifier is pure — the telemetry
//! subscriber feeds it per-request usage facts and counts the reasons into
//! the session totals, so cache spend becomes explainable: an idle gap
//! past the retention TTL, a changed prompt prefix (model, tools, or
//! system prompt — compared by fingerprint, never by content), or a drop
//! at or below the noise floor.
//!
//! Semantics follow the TS `cache-stats.ts` scan: the previous request's
//! prompt size is the cacheable baseline, a compaction legitimately
//! changes the context (the baseline resets), and a provider that never
//! reported cache activity classifies nothing.

use std::hash::{DefaultHasher, Hash, Hasher};

/// Prompt-cache retention: idle gaps longer than this likely expired the
/// cache (Anthropic's default TTL is five minutes; TS `CACHE_TTL_MS`). The
/// classification uses the request's own lifetime when it is known
/// ([`cache_ttl_ms_for`]); this is the default.
pub(crate) const CACHE_TTL_MS: u64 = 5 * 60 * 1_000;

/// The long-retention lifetime assumed when the model's own lifetime is
/// unknown (`PI_CACHE_RETENTION=long` writes one-hour entries).
const LONG_CACHE_TTL_MS: u64 = 60 * 60 * 1_000;

/// The lifetime of the cache entry a request against `model` with
/// `retention` writes: the model's known tier when there is one, else one
/// hour for long retention and [`CACHE_TTL_MS`] otherwise.
#[must_use]
pub(crate) fn cache_ttl_ms_for(
    model: &pa_types::ai::Model,
    retention: Option<pa_types::ai::CacheRetention>,
) -> u64 {
    pa_ai::cache_warming::prompt_cache_ttl_ms(model, retention).unwrap_or_else(|| {
        let long = match retention {
            Some(retention) => retention == pa_types::ai::CacheRetention::Long,
            None => std::env::var("PI_CACHE_RETENTION").as_deref() == Ok("long"),
        };
        if long {
            LONG_CACHE_TTL_MS
        } else {
            CACHE_TTL_MS
        }
    })
}

/// Per-request drops at or below this are cache-breakpoint granularity
/// noise (TS `NOISE_FLOOR_TOKENS`).
pub(crate) const CACHE_NOISE_FLOOR_TOKENS: u64 = 1_024;

/// The cache-relevant facts of one request. `prefix` is a fingerprint,
/// never content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RequestFacts {
    /// The whole prompt: input plus both cache buckets.
    pub(crate) prompt_tokens: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
    pub(crate) input: u64,
    /// When the request answered, in millis since the epoch.
    pub(crate) at_ms: u64,
    /// The fingerprint of the prefix the request was served with.
    pub(crate) prefix: u64,
}

impl RequestFacts {
    /// The facts of one request from its usage. The idle gap and the
    /// prefix come from the observer.
    #[must_use]
    pub(crate) fn from_usage(usage: &pa_agent::types::Usage, at_ms: u64, prefix: u64) -> Self {
        RequestFacts {
            prompt_tokens: usage
                .input
                .saturating_add(usage.cache_read)
                .saturating_add(usage.cache_write),
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            input: usage.input,
            at_ms,
            prefix,
        }
    }
}

/// The previous request a miss is measured against (TS `PreviousRequest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreviousRequest {
    prompt_tokens: u64,
    at_ms: u64,
    prefix: u64,
    /// Sticky: some request in this scan segment reported cache activity.
    /// Distinguishes a total miss on a cache-read-only provider from a
    /// provider that never reports caching at all.
    reported_cache: bool,
}

impl PreviousRequest {
    /// Whether some request in this scan segment reported cache activity
    /// (the sticky flag the classifier reads).
    #[must_use]
    pub(crate) fn reported_cache(&self) -> bool {
        self.reported_cache
    }

    /// The baseline the next request is measured against. A zero-prompt
    /// request is no baseline.
    #[must_use]
    pub(crate) fn from(facts: &RequestFacts, sticky_reported: bool) -> Option<PreviousRequest> {
        (facts.prompt_tokens > 0).then_some(PreviousRequest {
            prompt_tokens: facts.prompt_tokens,
            at_ms: facts.at_ms,
            prefix: facts.prefix,
            reported_cache: sticky_reported || facts.cache_read + facts.cache_write > 0,
        })
    }
}

/// Why a request's cache read dropped versus the previous request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheMissReason {
    /// The prefix identity (model, tools, system prompt) changed: a
    /// definite cause, so it outranks the idle guess.
    PrefixChanged,
    /// The idle gap since the previous request exceeds the retention TTL:
    /// the likely cause.
    TtlGap,
    /// A drop at or below the noise floor: breakpoint granularity, not a
    /// real miss.
    Noise,
    /// A drop above the floor with the same prefix and a short idle gap:
    /// real re-billed tokens the observable causes do not explain.
    Unattributed,
}

/// One classified drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClassifiedMiss {
    pub(crate) reason: CacheMissReason,
    /// Prompt tokens that were in the previous request's prompt but were
    /// not read from cache.
    pub(crate) missed_tokens: u64,
}

/// Classify one request against the previous. `None` when nothing is
/// comparable or nothing dropped: the first request, a zero prompt, a
/// provider that never reported cache activity, or a full read-back.
pub(crate) fn classify(
    prev: Option<&PreviousRequest>,
    current: &RequestFacts,
    ttl_ms: u64,
) -> Option<ClassifiedMiss> {
    let prev = prev?;
    if current.prompt_tokens == 0 {
        return None;
    }
    if current.cache_read + current.cache_write == 0 && !prev.reported_cache {
        return None;
    }
    let missed = prev
        .prompt_tokens
        .min(current.prompt_tokens)
        .saturating_sub(current.cache_read);
    if missed == 0 {
        return None;
    }
    let reason = if missed <= CACHE_NOISE_FLOOR_TOKENS {
        CacheMissReason::Noise
    } else if current.prefix != prev.prefix {
        CacheMissReason::PrefixChanged
    } else if current
        .at_ms
        .saturating_sub(prev.at_ms)
        .max(prev.at_ms.saturating_sub(current.at_ms))
        > ttl_ms
    {
        CacheMissReason::TtlGap
    } else {
        CacheMissReason::Unattributed
    };
    Some(ClassifiedMiss {
        reason,
        missed_tokens: missed,
    })
}

/// The order-insensitive fingerprint of everything but the model in the
/// cached prefix: the tool roster and the system prompt. Content never
/// leaves this module; only the hash does.
#[must_use]
pub(crate) fn static_prefix_fingerprint(tools: &[&str], system_prompt: &str) -> u64 {
    let mut sorted = tools.to_vec();
    sorted.sort_unstable();
    let mut hasher = DefaultHasher::new();
    for tool in sorted {
        tool.hash(&mut hasher);
    }
    system_prompt.hash(&mut hasher);
    hasher.finish()
}

/// The full prefix fingerprint of one request's serving configuration.
#[must_use]
pub(crate) fn request_prefix_fingerprint(model_key: &str, static_hash: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    model_key.hash(&mut hasher);
    static_hash.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(prompt: u64, cache_read: u64, at_ms: u64, prefix: u64) -> RequestFacts {
        RequestFacts {
            prompt_tokens: prompt,
            cache_read,
            cache_write: 0,
            input: prompt - cache_read,
            at_ms,
            prefix,
        }
    }

    fn prev(facts: &RequestFacts) -> Option<PreviousRequest> {
        PreviousRequest::from(facts, false)
    }

    #[test]
    fn the_first_request_classifies_nothing() {
        assert_eq!(classify(None, &facts(100_000, 0, 0, 1), CACHE_TTL_MS), None);
    }

    #[test]
    fn a_full_read_back_classifies_nothing() {
        let baseline = facts(100_000, 100_000, 0, 1);
        let previous = prev(&baseline).unwrap();
        // Everything the previous prompt held comes back from cache.
        assert_eq!(
            classify(
                Some(&previous),
                &facts(105_000, 100_000, 10_000, 1),
                CACHE_TTL_MS
            ),
            None
        );
    }

    #[test]
    fn a_small_drop_is_noise() {
        // A warm baseline: the previous request read its prompt from cache.
        let baseline = facts(100_000, 100_000, 0, 1);
        let previous = prev(&baseline).unwrap();
        assert_eq!(
            classify(
                Some(&previous),
                &facts(100_500, 99_000, 10_000, 1),
                CACHE_TTL_MS
            ),
            Some(ClassifiedMiss {
                reason: CacheMissReason::Noise,
                missed_tokens: 1_000,
            })
        );
        // Exactly the floor is still noise.
        assert_eq!(
            classify(
                Some(&previous),
                &facts(100_000, 100_000 - CACHE_NOISE_FLOOR_TOKENS, 10, 1),
                CACHE_TTL_MS
            )
            .map(|miss| miss.reason),
            Some(CacheMissReason::Noise)
        );
    }

    #[test]
    fn an_idle_gap_past_the_ttl_is_the_likely_cause() {
        let baseline = facts(100_000, 100_000, 0, 1);
        let previous = prev(&baseline).unwrap();
        // Ten minutes later, same prefix, the read dropped.
        assert_eq!(
            classify(
                Some(&previous),
                &facts(110_000, 0, 10 * 60 * 1_000, 1),
                CACHE_TTL_MS
            ),
            Some(ClassifiedMiss {
                reason: CacheMissReason::TtlGap,
                missed_tokens: 100_000,
            })
        );
        // Just inside the TTL with the same prefix: unattributed.
        assert_eq!(
            classify(
                Some(&previous),
                &facts(110_000, 0, CACHE_TTL_MS, 1),
                CACHE_TTL_MS
            )
            .map(|miss| miss.reason),
            Some(CacheMissReason::Unattributed)
        );
        // Under a one-hour (long-retention) lifetime the same ten-minute gap
        // cannot have expired the entry: it is unattributed, not a TTL gap.
        assert_eq!(
            classify(
                Some(&previous),
                &facts(110_000, 0, 10 * 60 * 1_000, 1),
                LONG_CACHE_TTL_MS
            )
            .map(|miss| miss.reason),
            Some(CacheMissReason::Unattributed)
        );
    }

    #[test]
    fn the_ttl_follows_the_request_retention_and_the_model_tier() {
        let mut model: pa_types::ai::Model = serde_json::from_value(serde_json::json!({
            "id": "m", "name": "m", "api": "test-api", "provider": "test",
            "baseUrl": "http://localhost", "reasoning": false, "input": ["text"],
            "cost": {"input": 1, "output": 1, "cacheRead": 0.1, "cacheWrite": 1.25},
            "contextWindow": 1000, "maxTokens": 100
        }))
        .unwrap();
        assert_eq!(
            [
                cache_ttl_ms_for(&model, Some(pa_types::ai::CacheRetention::Short)),
                cache_ttl_ms_for(&model, Some(pa_types::ai::CacheRetention::Long)),
            ],
            [CACHE_TTL_MS, LONG_CACHE_TTL_MS]
        );
        model.prompt_cache = Some(pa_types::ai::ModelPromptCache {
            short: Some(120),
            long: Some(7_200),
        });
        assert_eq!(
            [
                cache_ttl_ms_for(&model, Some(pa_types::ai::CacheRetention::Short)),
                cache_ttl_ms_for(&model, Some(pa_types::ai::CacheRetention::Long)),
            ],
            [120_000, 7_200_000]
        );
    }

    #[test]
    fn a_changed_prefix_outranks_the_idle_guess() {
        let baseline = facts(100_000, 100_000, 0, 1);
        let previous = prev(&baseline).unwrap();
        // A different prefix AND a TTL-sized gap: the prefix change is the
        // definite cause.
        assert_eq!(
            classify(
                Some(&previous),
                &facts(110_000, 0, 10 * 60 * 1_000, 2),
                CACHE_TTL_MS
            )
            .map(|miss| miss.reason),
            Some(CacheMissReason::PrefixChanged)
        );
        // A different prefix with a short gap classifies the same way.
        assert_eq!(
            classify(Some(&previous), &facts(110_000, 0, 1_000, 2), CACHE_TTL_MS)
                .map(|miss| miss.reason),
            Some(CacheMissReason::PrefixChanged)
        );
    }

    #[test]
    fn providers_that_never_report_cache_classify_nothing() {
        // The previous request also reported no cache activity: nothing is
        // comparable (an OpenAI-style provider without cache markers).
        let baseline = facts(100_000, 0, 0, 1);
        let previous = prev(&baseline).unwrap();
        assert_eq!(
            classify(Some(&previous), &facts(110_000, 0, 10, 1), CACHE_TTL_MS),
            None
        );
        // A zero-prompt request is no baseline either.
        assert_eq!(prev(&facts(0, 0, 0, 1)), None);
    }

    #[test]
    fn a_cache_read_only_provider_classifies_a_total_miss() {
        // The sticky flag: earlier requests reported cache activity, so a
        // zero-cache turn is a total miss, not an unreported provider.
        let warm = facts(100_000, 90_000, 0, 1);
        let previous = PreviousRequest::from(&warm, false).unwrap();
        assert!(previous.reported_cache);
        assert_eq!(
            classify(Some(&previous), &facts(110_000, 0, 10, 1), CACHE_TTL_MS),
            Some(ClassifiedMiss {
                reason: CacheMissReason::Unattributed,
                missed_tokens: 100_000,
            })
        );
    }

    #[test]
    fn the_fingerprint_ignores_tool_order_and_covers_the_prompt() {
        let a = static_prefix_fingerprint(&["ipython", "edit"], "system one");
        let b = static_prefix_fingerprint(&["edit", "ipython"], "system one");
        assert_eq!(a, b, "tool order does not change the prefix");
        assert_ne!(
            a,
            static_prefix_fingerprint(&["edit", "ipython"], "system two")
        );
        assert_ne!(a, static_prefix_fingerprint(&["edit"], "system one"));
        // The model key joins the request-level fingerprint.
        assert_ne!(
            request_prefix_fingerprint("p/m1", a),
            request_prefix_fingerprint("p/m2", a)
        );
        assert_ne!(
            request_prefix_fingerprint("p/m1", a),
            request_prefix_fingerprint(
                "p/m1",
                static_prefix_fingerprint(&["edit", "ipython"], "system two")
            )
        );
    }

    #[test]
    fn a_synthetic_session_classifies_each_reason_once() {
        // A whole-value scan over a synthetic sequence: a warm start, a
        // noisy turn, a model switch, an idle gap, and one unexplained
        // drop — each lands in its own bucket with its own tokens.
        let prefix_a =
            request_prefix_fingerprint("p/m1", static_prefix_fingerprint(&["ipython"], "s"));
        let prefix_b =
            request_prefix_fingerprint("p/m2", static_prefix_fingerprint(&["ipython"], "s"));
        let requests = [
            facts(100_000, 0, 0, prefix_a),
            facts(100_400, 99_600, 30_000, prefix_a),
            facts(110_000, 0, 60_000, prefix_b),
            facts(120_000, 0, 60_000 + 10 * 60 * 1_000, prefix_b),
            facts(130_000, 0, 60_000 + 10 * 60 * 1_000 + 5_000, prefix_b),
        ];
        let mut previous: Option<PreviousRequest> = None;
        let mut classified = Vec::new();
        for request in &requests {
            if let Some(miss) = classify(previous.as_ref(), request, CACHE_TTL_MS) {
                classified.push(miss);
            }
            previous = PreviousRequest::from(request, previous.is_some_and(|p| p.reported_cache))
                .or(previous);
        }
        assert_eq!(
            classified,
            vec![
                ClassifiedMiss {
                    reason: CacheMissReason::Noise,
                    missed_tokens: 400
                },
                ClassifiedMiss {
                    reason: CacheMissReason::PrefixChanged,
                    missed_tokens: 100_400
                },
                ClassifiedMiss {
                    reason: CacheMissReason::TtlGap,
                    missed_tokens: 110_000
                },
                ClassifiedMiss {
                    reason: CacheMissReason::Unattributed,
                    missed_tokens: 120_000
                },
            ]
        );
    }
}
