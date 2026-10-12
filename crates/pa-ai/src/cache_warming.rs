//! The model-side vocabulary of prompt-cache warming (TS
//! `packages/coding-agent/src/core/cache-warmer.ts`): the lifetime of the
//! cache entry a request writes, and whether replaying that request with a
//! one-token output cap is safe. The warmer service itself (scheduling,
//! economics, refresh) lives in pa-core; this module only answers
//! model-level questions.
//!
//! Lifetimes come from the catalog's `promptCache` tiers. Until the catalog
//! carries them (neither the live catalog nor the compiled fallback does
//! today), models served by Anthropic's or `OpenAI`'s own endpoints use
//! built-in defaults that mirror what our providers write on the wire — a
//! Rust-side default, superseded by any catalog value.

use crate::types::SimpleStreamOptions;
use pa_types::ai::{CacheRetention, Model, ModelThinkingLevel};

/// Lifetime of the prompt cache entry a request writes, from the model's
/// `promptCache` tier for the retention the request used (`getPromptCacheTtlMs`).
/// `None` when the request disabled caching, or the model has no lifetime
/// for that tier — the provider's cache behavior is unknown, so the entry is
/// never warmed.
///
/// A `None` retention resolves through `PI_CACHE_RETENTION` exactly like
/// every provider's `resolve_cache_retention`: `long` selects the long
/// tier, anything else the short one.
#[must_use]
pub fn prompt_cache_ttl_ms(model: &Model, retention: Option<CacheRetention>) -> Option<u64> {
    let retention = retention.unwrap_or_else(|| {
        if std::env::var("PI_CACHE_RETENTION").as_deref() == Ok("long") {
            CacheRetention::Long
        } else {
            CacheRetention::Short
        }
    });
    prompt_cache_ttl_ms_for_retention(model, retention)
}

/// The per-tier resolution behind [`prompt_cache_ttl_ms`]: seconds from the
/// wire schema (`ModelPromptCache`), milliseconds out. When the catalog
/// carries a `promptCache` map, a missing tier means unknown, so `None`;
/// without one, [`default_prompt_cache_seconds`] applies.
#[must_use]
pub fn prompt_cache_ttl_ms_for_retention(model: &Model, retention: CacheRetention) -> Option<u64> {
    let seconds = match (retention, model.prompt_cache) {
        (CacheRetention::None, _) => return None,
        (CacheRetention::Short, Some(cache)) => cache.short,
        (CacheRetention::Long, Some(cache)) => cache.long,
        (CacheRetention::Short | CacheRetention::Long, None) => {
            default_prompt_cache_seconds(model, retention)
        }
    };
    seconds.map(|seconds| seconds * 1_000)
}

/// The default (five-minute) prompt-cache lifetime of Anthropic's and
/// `OpenAI`'s own endpoints, in seconds.
const DEFAULT_SHORT_CACHE_SECONDS: u64 = 300;
/// Anthropic's one-hour entry, written when the request sends
/// `cache_control.ttl: "1h"`.
const ANTHROPIC_LONG_CACHE_SECONDS: u64 = 3_600;
/// `OpenAI`'s extended entry, written when the request sends
/// `prompt_cache_retention: "24h"`.
const OPENAI_LONG_CACHE_SECONDS: u64 = 86_400;

/// Built-in lifetimes for models the catalog gives no `promptCache` (a
/// Rust-side default until the catalog carries the field). Only the
/// providers' own endpoints qualify: an Anthropic- or OpenAI-compatible
/// third-party host has unknown cache behavior, so `None`. The long tier
/// mirrors exactly when the provider sends the long-retention field;
/// otherwise a long request writes a default-lifetime entry.
fn default_prompt_cache_seconds(model: &Model, retention: CacheRetention) -> Option<u64> {
    let long = retention == CacheRetention::Long;
    match model.api.as_str() {
        crate::providers::anthropic::API_ANTHROPIC_MESSAGES
            if model.base_url.contains("api.anthropic.com") =>
        {
            let sends_one_hour = long
                && crate::providers::anthropic::get_anthropic_compat(model)
                    .supports_long_cache_retention;
            Some(if sends_one_hour {
                ANTHROPIC_LONG_CACHE_SECONDS
            } else {
                DEFAULT_SHORT_CACHE_SECONDS
            })
        }
        "openai-completions" if model.base_url.contains("api.openai.com") => {
            let sends_24h = long
                && crate::providers::openai_completions::get_compat(model)
                    .supports_long_cache_retention;
            Some(if sends_24h {
                OPENAI_LONG_CACHE_SECONDS
            } else {
                DEFAULT_SHORT_CACHE_SECONDS
            })
        }
        "openai-responses" if model.base_url.contains("api.openai.com") => {
            let sends_24h = long
                && crate::providers::openai_responses::get_responses_compat(model)
                    .supports_long_cache_retention;
            Some(if sends_24h {
                OPENAI_LONG_CACHE_SECONDS
            } else {
                DEFAULT_SHORT_CACHE_SECONDS
            })
        }
        _ => None,
    }
}

/// Whether replaying the request with a one-token output cap leaves its
/// cache entry untouched (`isReplayable`). Anthropic's budget-based
/// thinking — Claude models without adaptive thinking — derives
/// `budget_tokens` from `max_tokens`: the replay would get a different
/// budget, which Anthropic keys the message cache on, and the model could
/// still think for thousands of tokens. Any reasoning level set at all
/// (including `Off`) counts, mirroring the TS truthiness check on
/// `options.reasoning`.
#[must_use]
pub fn is_replayable(model: &Model, reasoning: Option<ModelThinkingLevel>) -> bool {
    if reasoning.is_none()
        || model.api.as_str() != crate::providers::anthropic::API_ANTHROPIC_MESSAGES
    {
        return true;
    }
    crate::providers::anthropic::supports_adaptive_thinking(&model.id)
}

/// The [`is_replayable`] view of a full simple-stream request: the reasoning
/// level its options carry.
#[must_use]
pub fn request_is_replayable(model: &Model, options: Option<&SimpleStreamOptions>) -> bool {
    is_replayable(model, options.and_then(|options| options.reasoning))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_types::ai::ModelPromptCache;
    use pa_types::JsNumber;

    fn model(api: &str, id: &str, prompt_cache: Option<ModelPromptCache>) -> Model {
        Model {
            id: id.to_string(),
            name: id.to_string(),
            api: pa_types::ai::Api::from(api),
            provider: "test".to_string(),
            base_url: "https://example.invalid".to_string(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![pa_types::ai::ModelInput::Text],
            cost: pa_types::ai::ModelCost {
                input: JsNumber::from(1.0),
                output: JsNumber::from(2.0),
                cache_read: JsNumber::from(0.1),
                cache_write: JsNumber::from(1.25),
            },
            context_window: 128_000,
            max_tokens: 4_096,
            max_tokens_explicit: false,
            featured: None,
            prompt_cache,
            headers: None,
            compat: None,
        }
    }

    fn tiers(short: Option<u64>, long: Option<u64>) -> ModelPromptCache {
        ModelPromptCache { short, long }
    }

    #[test]
    fn ttl_comes_from_the_retention_tier_in_milliseconds() {
        let model = model(
            "anthropic",
            "claude-opus-4-5",
            Some(tiers(Some(300), Some(3_600))),
        );
        assert_eq!(
            prompt_cache_ttl_ms_for_retention(&model, CacheRetention::Short),
            Some(300_000)
        );
        assert_eq!(
            prompt_cache_ttl_ms_for_retention(&model, CacheRetention::Long),
            Some(3_600_000)
        );
    }

    #[test]
    fn a_missing_tier_or_missing_retention_means_unknown() {
        let short_only = model(
            "anthropic-messages",
            "claude-opus-4-5",
            Some(tiers(Some(300), None)),
        );
        assert_eq!(
            prompt_cache_ttl_ms_for_retention(&short_only, CacheRetention::Long),
            None,
            "a missing long tier is an unknown lifetime, never a fallback to short"
        );
        let no_cache = model("anthropic-messages", "claude-opus-4-5", None);
        assert_eq!(
            prompt_cache_ttl_ms_for_retention(&no_cache, CacheRetention::Short),
            None
        );
        let cached = model(
            "anthropic-messages",
            "claude-opus-5",
            Some(tiers(Some(300), None)),
        );
        assert_eq!(
            prompt_cache_ttl_ms(&cached, Some(CacheRetention::None)),
            None
        );
    }

    #[test]
    fn an_explicit_retention_selects_its_tier() {
        let model = model(
            "anthropic",
            "claude-opus-4-5",
            Some(tiers(Some(300), Some(3_600))),
        );
        assert_eq!(
            prompt_cache_ttl_ms(&model, Some(CacheRetention::Long)),
            Some(3_600_000)
        );
    }

    fn hosted(api: &str, base_url: &str, id: &str) -> Model {
        Model {
            base_url: base_url.to_string(),
            ..model(api, id, None)
        }
    }

    #[test]
    fn providers_own_endpoints_have_default_lifetimes_until_the_catalog_has_them() {
        let anthropic = hosted(
            "anthropic-messages",
            "https://api.anthropic.com",
            "claude-opus-4-5",
        );
        assert_eq!(
            [
                prompt_cache_ttl_ms_for_retention(&anthropic, CacheRetention::Short),
                prompt_cache_ttl_ms_for_retention(&anthropic, CacheRetention::Long),
            ],
            [Some(300_000), Some(3_600_000)]
        );
        let openai = hosted("openai-completions", "https://api.openai.com/v1", "gpt-4");
        assert_eq!(
            [
                prompt_cache_ttl_ms_for_retention(&openai, CacheRetention::Short),
                prompt_cache_ttl_ms_for_retention(&openai, CacheRetention::Long),
            ],
            [Some(300_000), Some(86_400_000)]
        );
        let responses = hosted("openai-responses", "https://api.openai.com/v1", "gpt-5");
        assert_eq!(
            prompt_cache_ttl_ms_for_retention(&responses, CacheRetention::Long),
            Some(86_400_000)
        );
        // Third-party hosts of the same APIs have unknown cache behavior.
        for third_party in [
            hosted(
                "anthropic-messages",
                "https://api.moonshot.ai/anthropic",
                "kimi",
            ),
            hosted("openai-completions", "https://api.pinference.ai/v1", "m"),
        ] {
            assert_eq!(
                prompt_cache_ttl_ms_for_retention(&third_party, CacheRetention::Short),
                None
            );
        }
        // Catalog tiers win, including a catalog map without a tier.
        let catalog = Model {
            prompt_cache: Some(tiers(Some(120), None)),
            ..anthropic
        };
        assert_eq!(
            [
                prompt_cache_ttl_ms_for_retention(&catalog, CacheRetention::Short),
                prompt_cache_ttl_ms_for_retention(&catalog, CacheRetention::Long),
            ],
            [Some(120_000), None]
        );
    }

    #[test]
    fn budget_based_anthropic_reasoning_is_not_replayable() {
        let claude = model("anthropic-messages", "claude-opus-4-5", None);
        assert!(!is_replayable(&claude, Some(ModelThinkingLevel::High)));
        // The TS truthiness check treats any reasoning value — Off included
        // — as reasoning being configured, so the budget concern applies.
        assert!(!is_replayable(&claude, Some(ModelThinkingLevel::Off)));
        assert!(is_replayable(&claude, None));
    }

    #[test]
    fn adaptive_thinking_anthropic_and_other_apis_replay() {
        let adaptive = model("anthropic-messages", "claude-opus-4-6", None);
        assert!(is_replayable(&adaptive, Some(ModelThinkingLevel::High)));
        let openai = model("openai-completions", "gpt-test", None);
        assert!(is_replayable(&openai, Some(ModelThinkingLevel::High)));
    }
}
