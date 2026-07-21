//! Token -> notional cost using a per-model pricing table.
//!
//! The Max subscription is a flat fee, so these numbers are a *notional*
//! "equivalent API cost" - what the same tokens would bill at published
//! pay-as-you-go rates - not an actual charge.
//!
//! Prices are USD per million tokens. Source: Anthropic published pricing as of
//! 2026-07 (input/output from the model catalog). Cache classes follow the
//! standard multipliers on the input price: cache-write = 1.25x (5-minute
//! ephemeral, which dominates the real transcripts), cache-read = 0.10x.
//! Keyed by a substring of the model id so `claude-opus-4-8`,
//! `claude-opus-4-7`, etc. all resolve to the Opus row.

/// USD per million tokens, per token class.
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

/// Look up pricing by matching known substrings of the model id.
pub fn pricing_for(model: &str) -> ModelPricing {
    let m = model.to_ascii_lowercase();
    // Order matters only where substrings could overlap; these do not.
    if m.contains("opus") {
        ModelPricing {
            input: 5.0,
            output: 25.0,
            cache_write: 6.25,
            cache_read: 0.5,
        }
    } else if m.contains("sonnet") {
        ModelPricing {
            input: 3.0,
            output: 15.0,
            cache_write: 3.75,
            cache_read: 0.3,
        }
    } else if m.contains("haiku") {
        ModelPricing {
            input: 1.0,
            output: 5.0,
            cache_write: 1.25,
            cache_read: 0.1,
        }
    } else if m.contains("fable") || m.contains("mythos") {
        ModelPricing {
            input: 10.0,
            output: 50.0,
            cache_write: 12.5,
            cache_read: 1.0,
        }
    } else {
        // Sane fallback for unknown / future ids: mid-tier (Sonnet) rates.
        ModelPricing {
            input: 3.0,
            output: 15.0,
            cache_write: 3.75,
            cache_read: 0.3,
        }
    }
}

/// Notional USD cost for one model's token totals.
pub fn cost_for(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
    let p = pricing_for(model);
    let per_million = |tokens: u64, price: f64| (tokens as f64) / 1_000_000.0 * price;
    per_million(input, p.input)
        + per_million(output, p.output)
        + per_million(cache_read, p.cache_read)
        + per_million(cache_write, p.cache_write)
}
