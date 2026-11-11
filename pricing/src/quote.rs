//! Rate resolution: turning a [`ModelKey`] and a timestamp into four rates.

use chrono::{DateTime, Utc};

use crate::book::{BookTier, PriceBook, PriceRow, default_row};
use crate::model::ModelKey;
use crate::ratio::CacheRatios;

/// Which layer answered. Recorded on every quote so a stored cost can be
/// explained, and so an estimate is never mistaken for a looked-up rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    /// Matched the model, as named, in a synced price book.
    Exact,
    /// Matched the model family in a synced book, after dropping a minor version.
    Family,
    /// The synced book's newest rate, for a call older than its price history.
    Backdated,
    /// Matched the offline list-price table.
    Static,
    /// No rate found; inferred from the vendor's cache-pricing convention.
    Ratio,
    /// No rate found at any layer.
    Default,
}

impl PriceSource {
    /// Whether a quote from this layer is a lookup or a guess. Drives
    /// [`PriceQuote::estimated`], which surfaces report so a dashboard can say
    /// "$412, of which $38 estimated" instead of presenting a guess as fact.
    fn is_estimate(self) -> bool {
        matches!(
            self,
            Self::Family | Self::Backdated | Self::Static | Self::Ratio | Self::Default
        )
    }
}

/// Four resolved rates in USD per million tokens, with their provenance.
///
/// All four are always present. A missing rate is never represented as zero:
/// zero is a real price that some models genuinely have (OpenAI does not charge
/// for cache writes), and conflating "free" with "unknown" is what let 92.8% of
/// gateway calls be booked at $0.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceQuote {
    pub input_per_1m: f64,
    pub output_per_1m: f64,
    pub cache_read_per_1m: f64,
    pub cache_creation_per_1m: f64,
    pub cache_creation_1h_per_1m: Option<f64>,
    /// Where the input and output rates came from.
    pub source: PriceSource,
    /// Where the cache rates came from. Often a different, lower layer: 81% of
    /// rows in the live book carry input and output prices but no cache prices.
    pub cache_source: PriceSource,
    pub estimated: bool,
    /// The name the rates were found under, for auditing a stored cost.
    pub resolved_as: String,
}

/// Resolve rates for `key` as they stood at `at`.
///
/// Books are searched in order and their answers **merged field by field**: a
/// row that carries input and output but no cache prices contributes what it has
/// and the search continues for the cache pair alone. Short-circuiting instead —
/// which is what `DbPricing` does today — lets a partial database row shadow a
/// complete static entry, and is why `claude-3-5-sonnet` cache reads are billed
/// at 10x their true rate.
///
/// `at` is the timestamp of the call being priced, not now: `model_pricing`
/// carries real price history, and re-pricing an old call at today's rates
/// silently rewrites it.
pub async fn quote(
    books: &[&dyn PriceBook],
    key: &ModelKey,
    at: DateTime<Utc>,
    ratios: &CacheRatios,
) -> PriceQuote {
    let mut base: Option<(PriceRow, PriceSource, String)> = None;
    let mut read = None;
    let mut creation = None;
    let mut unqualified = false;

    'search: for book in books {
        for probe in lookup_plan(key) {
            let Some(row) = probe.run(*book, key, at).await else {
                continue;
            };
            let source = attribute(book.tier(), probe.name_match);
            if base.is_none() {
                base = Some((row, source, probe.name.clone()));
                unqualified |= !probe.qualified;
            }
            if read.is_none()
                && let Some(rate) = row.cache_read_per_1m
            {
                read = Some((rate, source));
                unqualified |= !probe.qualified;
            }
            if creation.is_none()
                && let Some(rate) = row.cache_creation_per_1m
            {
                creation = Some((rate, source));
                unqualified |= !probe.qualified;
            }
            if base.is_some() && read.is_some() && creation.is_some() {
                break 'search;
            }
        }
    }

    let (row, source, resolved_as) =
        base.unwrap_or_else(|| (default_row(), PriceSource::Default, key.canonical.clone()));

    let inferred = ratios.rates_for(key.vendor, row.input_per_1m);
    let (read_rate, read_source) = read.unwrap_or((inferred.0, PriceSource::Ratio));
    let (creation_rate, creation_source) = creation.unwrap_or((inferred.1, PriceSource::Ratio));
    let cache_source = if read_source.is_estimate() {
        read_source
    } else {
        creation_source
    };

    PriceQuote {
        input_per_1m: row.input_per_1m,
        output_per_1m: row.output_per_1m,
        cache_read_per_1m: read_rate,
        cache_creation_per_1m: creation_rate,
        // A duration-specific rate must come from the same matched price row.
        cache_creation_1h_per_1m: row.cache_creation_1h_per_1m,
        source,
        cache_source,
        estimated: unqualified
            || source.is_estimate()
            || read_source.is_estimate()
            || creation_source.is_estimate(),
        resolved_as,
    }
}

/// How closely a candidate name matched — independent of which book answered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NameMatch {
    /// The model as named, once normalized.
    AsNamed,
    /// Its family, after dropping a minor version.
    Family,
}

/// One attempt at finding a row: a name to look up, and whether to require the
/// caller's provider to match.
struct Probe {
    name: String,
    name_match: NameMatch,
    /// Require `(provider, name)`; otherwise match on the name alone.
    qualified: bool,
}

impl Probe {
    async fn run(
        &self,
        book: &dyn PriceBook,
        key: &ModelKey,
        at: DateTime<Utc>,
    ) -> Option<PriceRow> {
        match (&key.provider, self.qualified) {
            (Some(provider), true) => book.exact(provider, &self.name, at).await,
            (None, true) => None,
            (_, false) => book.by_model(&self.name, at).await,
        }
    }
}

/// Every probe, in priority order.
///
/// Two orderings matter, and both were learned from real misses:
///
/// * **The reported name comes before the normalized one.** Prefix stripping is
///   a guess about what is routing noise, and on Bedrock it guesses wrong:
///   `openai.gpt-6-astra` *is* the model, and its row is keyed that way. Probing
///   the stripped name first matched an unrelated OpenAI row at $10/$50 while
///   the correct Bedrock row sat at $11/$55. The same ordering also prefers a
///   date-stamped row (`claude-3-5-sonnet-20241022`, which carries cache rates)
///   over the undated one (which does not).
/// * **Every provider-qualified probe comes before any unqualified one.**
///   Providers charge differently for the same model, so the right provider
///   under a looser name beats the right name under the wrong provider.
fn lookup_plan(key: &ModelKey) -> Vec<Probe> {
    let verbatim = key.model.trim().to_lowercase();
    let mut names = vec![(verbatim.clone(), NameMatch::AsNamed)];
    if key.canonical != verbatim {
        names.push((key.canonical.clone(), NameMatch::AsNamed));
    }
    if key.family != key.canonical && key.family != verbatim {
        names.push((key.family.clone(), NameMatch::Family));
    }

    let probe = |qualified: bool| {
        names.iter().cloned().map(move |(name, name_match)| Probe {
            name,
            name_match,
            qualified,
        })
    };
    if key.provider.is_some() {
        probe(true).chain(probe(false)).collect()
    } else {
        probe(false).collect()
    }
}

/// A rate from an offline book is a fallback however well the name matched —
/// conflating the two is what would let a compiled-in list price be reported as
/// a looked-up one.
fn attribute(tier: BookTier, name_match: NameMatch) -> PriceSource {
    match (tier, name_match) {
        (BookTier::Offline, _) => PriceSource::Static,
        (BookTier::Backdated, _) => PriceSource::Backdated,
        (BookTier::Synced, NameMatch::AsNamed) => PriceSource::Exact,
        (BookTier::Synced, NameMatch::Family) => PriceSource::Family,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::StaticPriceBook;
    use crate::model::resolve_model;
    use async_trait::async_trait;
    use std::collections::HashMap;

    /// A book backed by a fixed map, standing in for `model_pricing`.
    struct FakeBook(HashMap<String, PriceRow>);

    impl FakeBook {
        fn new(rows: &[(&str, PriceRow)]) -> Self {
            Self(
                rows.iter()
                    .map(|(name, row)| ((*name).to_string(), *row))
                    .collect(),
            )
        }
    }

    #[async_trait]
    impl PriceBook for FakeBook {
        async fn exact(&self, _p: &str, model: &str, _at: DateTime<Utc>) -> Option<PriceRow> {
            self.0.get(model).copied()
        }
        async fn by_model(&self, model: &str, _at: DateTime<Utc>) -> Option<PriceRow> {
            self.0.get(model).copied()
        }
    }

    fn row(input: f64, output: f64, read: Option<f64>, creation: Option<f64>) -> PriceRow {
        PriceRow {
            input_per_1m: input,
            output_per_1m: output,
            cache_read_per_1m: read,
            cache_creation_per_1m: creation,
            cache_creation_1h_per_1m: None,
        }
    }

    async fn quote_for(
        books: &[&dyn PriceBook],
        provider: Option<&str>,
        model: &str,
    ) -> PriceQuote {
        quote(
            books,
            &resolve_model(provider, model),
            Utc::now(),
            &CacheRatios::measured(),
        )
        .await
    }

    #[tokio::test]
    async fn an_exact_row_is_used_whole() {
        let book = FakeBook::new(&[("claude-opus-5", row(15.0, 75.0, Some(1.50), Some(18.75)))]);
        let q = quote_for(&[&book], Some("anthropic"), "claude-opus-5").await;
        assert_eq!(q.input_per_1m, 15.0);
        assert_eq!(q.cache_read_per_1m, 1.50);
        assert_eq!(q.source, PriceSource::Exact);
        assert!(!q.estimated);
    }

    #[tokio::test]
    async fn a_partial_row_inherits_cache_rates_instead_of_shadowing_them() {
        // D2/D3: the live book has claude-3-5-sonnet with NULL cache columns.
        // Today that row shadows the static table and cache reads are billed at
        // the input rate — $3.00/1M against a true $0.30, a 10x over-charge.
        let db = FakeBook::new(&[("claude-3-5-sonnet", row(3.0, 15.0, None, None))]);
        let q = quote_for(
            &[&db, &StaticPriceBook],
            Some("anthropic"),
            "claude-3-5-sonnet",
        )
        .await;
        assert_eq!(q.input_per_1m, 3.0, "base rates must come from the DB row");
        assert_eq!(
            q.cache_read_per_1m, 0.30,
            "cache must be inherited, not shadowed"
        );
        assert_eq!(q.source, PriceSource::Exact);
        assert_eq!(q.cache_source, PriceSource::Static);
    }

    #[tokio::test]
    async fn a_minor_version_falls_back_to_its_family() {
        // D4: claude-opus-4-6 is a real coding-agent model with no row of its own.
        let db = FakeBook::new(&[("claude-opus-4", row(15.0, 75.0, Some(1.50), Some(18.75)))]);
        let q = quote_for(&[&db], Some("anthropic"), "claude-opus-4-6").await;
        assert_eq!(q.input_per_1m, 15.0);
        assert_eq!(q.cache_read_per_1m, 1.50);
        assert_eq!(q.source, PriceSource::Family);
        assert_eq!(q.resolved_as, "claude-opus-4");
    }

    #[tokio::test]
    async fn a_missing_cache_rate_is_inferred_never_left_at_the_input_rate() {
        let db = FakeBook::new(&[("claude-opus-5", row(15.0, 75.0, None, None))]);
        let q = quote_for(&[&db], Some("anthropic"), "claude-opus-5").await;
        assert_eq!(
            q.cache_read_per_1m, 1.50,
            "ratio must reproduce the true rate"
        );
        assert_ne!(q.cache_read_per_1m, q.input_per_1m);
        assert_eq!(q.cache_source, PriceSource::Ratio);
        assert!(q.estimated, "an inferred cache rate must be flagged");
    }

    #[tokio::test]
    async fn an_unknown_model_gets_a_default_rate_never_a_zero_one() {
        // D1: the SQL trigger returns NULL here, and 389 of 419 live gateway rows
        // are booked at $0 because of it.
        let empty = FakeBook::new(&[]);
        let q = quote_for(&[&empty], Some("aws-bedrock"), "openai.gpt-6-astra").await;
        assert!(q.input_per_1m > 0.0);
        assert!(q.output_per_1m > 0.0);
        assert!(q.cache_read_per_1m > 0.0);
        assert_eq!(q.source, PriceSource::Default);
        assert!(q.estimated);
    }

    #[tokio::test]
    async fn a_prefixed_model_id_is_probed_before_the_stripped_one() {
        // Bedrock's own name for the model is `openai.gpt-6-astra` at $11/$55.
        // Stripping `openai.` first matched an unrelated OpenAI row at $10/$55
        // while the correct row sat one probe away.
        let db = FakeBook::new(&[
            (
                "openai.gpt-6-astra",
                row(11.0, 55.0, Some(1.10), Some(13.75)),
            ),
            ("gpt-6-astra", row(10.0, 50.0, Some(1.00), Some(12.50))),
        ]);
        let q = quote_for(&[&db], Some("aws-bedrock"), "openai.gpt-6-astra").await;
        assert_eq!(q.input_per_1m, 11.0, "resolved as {}", q.resolved_as);
        assert_eq!(q.resolved_as, "openai.gpt-6-astra");
    }

    #[tokio::test]
    async fn a_provider_label_is_not_rewritten_before_lookup() {
        // The pricing sync writes rows under the router's own label, so the
        // reported label is the one the book is keyed by.
        let db = FakeBook::new(&[(
            "anthropic.claude-opus-4-6-v1:0",
            row(5.0, 25.0, Some(0.50), Some(6.25)),
        )]);
        let q = quote_for(
            &[&db],
            Some("aws-bedrock"),
            "anthropic.claude-opus-4-6-v1:0",
        )
        .await;
        assert_eq!(q.input_per_1m, 5.0, "Bedrock rate, not Anthropic's $15");
    }

    #[tokio::test]
    async fn a_dated_row_wins_over_the_undated_one_that_has_no_cache_rates() {
        // model_pricing carries both spellings; only the dated row is complete.
        let db = FakeBook::new(&[
            (
                "claude-3-5-sonnet-20241022",
                row(3.0, 15.0, Some(0.30), Some(3.75)),
            ),
            ("claude-3-5-sonnet", row(3.0, 15.0, None, None)),
        ]);
        let q = quote_for(&[&db], Some("anthropic"), "claude-3-5-sonnet-20241022").await;
        assert_eq!(q.cache_read_per_1m, 0.30);
        assert_eq!(q.cache_source, PriceSource::Exact);
    }

    #[tokio::test]
    async fn the_right_provider_beats_the_right_name_under_the_wrong_one() {
        // Providers charge differently for the same model, so every qualified
        // probe runs before any unqualified one.
        struct ByProvider;
        #[async_trait]
        impl PriceBook for ByProvider {
            async fn exact(&self, p: &str, model: &str, _at: DateTime<Utc>) -> Option<PriceRow> {
                (p == "aws-bedrock" && model == "claude-opus-4")
                    .then(|| row(5.0, 25.0, Some(0.5), Some(6.25)))
            }
            async fn by_model(&self, model: &str, _at: DateTime<Utc>) -> Option<PriceRow> {
                (model == "claude-opus-4-6").then(|| row(15.0, 75.0, Some(1.5), Some(18.75)))
            }
        }
        let q = quote_for(&[&ByProvider], Some("aws-bedrock"), "claude-opus-4-6").await;
        assert_eq!(q.input_per_1m, 5.0, "took the wrong provider's rate");
        assert_eq!(q.source, PriceSource::Family);
    }

    #[tokio::test]
    async fn the_database_wins_over_the_static_table() {
        let db = FakeBook::new(&[("gpt-4o", row(1.0, 2.0, Some(0.1), Some(0.0)))]);
        let q = quote_for(&[&db, &StaticPriceBook], Some("openai"), "gpt-4o").await;
        assert_eq!(
            q.input_per_1m, 1.0,
            "synced price must beat the offline table"
        );
    }

    #[tokio::test]
    async fn a_normalized_name_finds_a_row_written_under_the_plain_spelling() {
        let db = FakeBook::new(&[("claude-opus-4", row(15.0, 75.0, Some(1.5), Some(18.75)))]);
        let q = quote_for(
            &[&db],
            Some("aws-bedrock"),
            "us.anthropic.claude-opus-4-v1:0",
        )
        .await;
        assert_eq!(q.input_per_1m, 15.0);
    }

    #[tokio::test]
    async fn every_quote_carries_four_usable_rates() {
        let empty = FakeBook::new(&[]);
        for model in [
            "claude-opus-4-6",
            "gpt-6-astra",
            "glm-5.3",
            "totally-unknown",
        ] {
            let q = quote_for(&[&empty, &StaticPriceBook], None, model).await;
            for rate in [
                q.input_per_1m,
                q.output_per_1m,
                q.cache_read_per_1m,
                q.cache_creation_per_1m,
            ] {
                assert!(rate.is_finite() && rate >= 0.0, "{model} produced {rate}");
            }
            assert!(q.input_per_1m > 0.0, "{model} had a zero input rate");
        }
    }
}
