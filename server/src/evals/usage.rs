//! One run's attempts, totalled from the router's own usage rows.
//!
//! Every call the `eval` identity makes without a flow is its own `direct:<uuid>` row in
//! `trace_usage` (the router's `trace_key`), so a row is one call and its `metadata.pricing` is that
//! call's price provenance (`PricedCall::provenance`). The totals read the router's `router_*`
//! share, which is exactly what the router priced: the Tempo materializer never adds to a row the
//! router wrote for a call with no trace.

use axum::http::StatusCode;
use nasiko_pricing::PriceSource;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::ApiError;

/// The longest run id accepted: an attempt tag is at most 128 characters (the router's
/// `eval_tag`), and it is the run id, a `:` and at least one more character.
const MAX_RUN_LEN: usize = 126;
/// Attempts per page when the request names no limit.
const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 1000;

/// `GET /api/evals/usage` query. Strings, parsed here, so a bad value answers the JSON error
/// rather than axum's plain-text one.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct UsageParams {
    /// The run id: attempts tagged `<run>:…` are returned. Characters `A-Z a-z 0-9 : . _ / -`.
    pub run: Option<String>,
    /// `next_cursor` from the previous page.
    pub cursor: Option<String>,
    /// Attempts per page, 1-1000 (default 200).
    pub limit: Option<String>,
}

/// One attempt's totals.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct AttemptUsage {
    /// The attempt tag, as the eval runner sent it.
    pub attempt: String,
    /// Calls the router served for the attempt, failed ones included.
    pub calls: i64,
    /// Of `calls`, those that reached the provider and failed.
    pub failed_calls: i64,
    /// Every model that served a call, sorted.
    pub models: Vec<String>,
    /// Fresh prompt tokens, disjoint from the two cache counts.
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd: f64,
    /// The least certain price any call was given: `Exact` < `Family` < `Static` < `Ratio` <
    /// `Default`. A call with no price record counts as `Default`.
    pub worst_price_source: String,
    /// Calls whose cost is an estimate: priced at an inferred rate, with no price record, or failed
    /// with an unreported prompt (its cost may be understated).
    pub estimated_calls: i64,
}

/// A validated query: the run's tag range and the page.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RunQuery {
    /// Lowest tag of the run, `<run>:`.
    from: String,
    /// First tag past the run, `<run>;` (`;` is the byte after `:`).
    until: String,
    cursor: Option<String>,
    limit: i64,
}

impl RunQuery {
    pub(super) fn parse(params: UsageParams) -> Result<Self, ApiError> {
        let run = params
            .run
            .filter(|run| run.len() <= MAX_RUN_LEN && is_tag_text(run))
            .ok_or(ApiError(
                StatusCode::BAD_REQUEST,
                "invalid_run",
                "run must be 1-126 characters of A-Z a-z 0-9 : . _ / -",
            ))?;
        let cursor = match params.cursor {
            Some(cursor) if !is_tag_text(&cursor) => {
                return Err(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_cursor",
                    "cursor is not one this endpoint returned",
                ));
            }
            cursor => cursor,
        };
        let limit = match params.limit {
            None => DEFAULT_LIMIT,
            Some(raw) => raw
                .parse::<i64>()
                .ok()
                .filter(|n| (1..=MAX_LIMIT).contains(n))
                .ok_or(ApiError(
                    StatusCode::BAD_REQUEST,
                    "invalid_limit",
                    "limit must be 1-1000",
                ))?,
        };
        Ok(Self {
            from: format!("{run}:"),
            until: format!("{run};"),
            cursor,
            limit,
        })
    }
}

/// Non-empty, and only the characters an attempt tag may hold.
fn is_tag_text(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'_' | b'/' | b'-'))
}

/// Every attempt of the caller's run `run` (all pages).
pub(super) async fn run_usage(
    db: &PgPool,
    user_id: Uuid,
    run: &str,
) -> anyhow::Result<Vec<AttemptUsage>> {
    let mut all = Vec::new();
    let mut cursor = None;
    loop {
        let query = RunQuery::parse(UsageParams {
            run: Some(run.to_string()),
            cursor: cursor.clone(),
            limit: Some(MAX_LIMIT.to_string()),
        })
        .map_err(|e| anyhow::anyhow!("invalid run {run:?}: {}", e.2))?;
        let page = attempts(db, user_id, &query).await?;
        all.extend(page.attempts);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(all),
        }
    }
}

/// One page of a run's attempts.
pub(super) struct AttemptPage {
    pub attempts: Vec<AttemptUsage>,
    /// The last attempt returned, when more follow.
    pub next_cursor: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AttemptRow {
    attempt: String,
    calls: i64,
    failed_calls: i64,
    models: Vec<String>,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
    cost_usd: f64,
    price_sources: Vec<String>,
    estimated_calls: i64,
}

/// The caller's attempts of the run, in byte order of their tags, one page at a time.
///
/// The tag range is compared with the `text_pattern_ops` operators (`~>=~`, `~<~`, `~>~`), byte by
/// byte, so it is the same range under any collation and is served by
/// `idx_trace_usage_eval_attempt` (0113); `COLLATE "C"` orders the page the same way.
pub(super) async fn attempts(
    db: &PgPool,
    user_id: Uuid,
    query: &RunQuery,
) -> Result<AttemptPage, sqlx::Error> {
    let mut rows: Vec<AttemptRow> = sqlx::query_as(
        r#"SELECT eval_attempt AS attempt,
                  count(*) AS calls,
                  count(*) FILTER (WHERE router_error_count > 0) AS failed_calls,
                  COALESCE(array_agg(DISTINCT model) FILTER (WHERE model IS NOT NULL), '{}')
                      AS models,
                  sum(router_input_tokens)::bigint AS input_tokens,
                  sum(router_output_tokens)::bigint AS output_tokens,
                  sum(router_cache_read_tokens)::bigint AS cache_read_tokens,
                  sum(router_cache_creation_tokens)::bigint AS cache_creation_tokens,
                  sum(router_cost_usd)::float8 AS cost_usd,
                  array_agg(DISTINCT COALESCE(metadata->'pricing'->>'source', 'Default'))
                      AS price_sources,
                  count(*) FILTER (
                      WHERE COALESCE((metadata->'pricing'->>'estimated')::boolean, true)
                         OR router_estimated_input_tokens > 0
                  ) AS estimated_calls
           FROM trace_usage
           WHERE user_id = $1
             AND eval_attempt ~>=~ $2 AND eval_attempt ~<~ $3
             AND ($4::text IS NULL OR eval_attempt ~>~ $4)
           GROUP BY eval_attempt
           ORDER BY eval_attempt COLLATE "C"
           LIMIT $5"#,
    )
    .bind(user_id)
    .bind(&query.from)
    .bind(&query.until)
    .bind(query.cursor.as_deref())
    .bind(query.limit + 1)
    .fetch_all(db)
    .await?;
    let has_more = rows.len() as i64 > query.limit;
    rows.truncate(query.limit as usize);
    let attempts: Vec<AttemptUsage> = rows.into_iter().map(AttemptRow::into_usage).collect();
    let next_cursor = has_more
        .then(|| attempts.last().map(|a| a.attempt.clone()))
        .flatten();
    Ok(AttemptPage {
        attempts,
        next_cursor,
    })
}

impl AttemptRow {
    fn into_usage(self) -> AttemptUsage {
        let mut models = self.models;
        models.sort();
        AttemptUsage {
            attempt: self.attempt,
            calls: self.calls,
            failed_calls: self.failed_calls,
            models,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            cost_usd: self.cost_usd,
            worst_price_source: format!("{:?}", worst_source(&self.price_sources)),
            estimated_calls: self.estimated_calls,
        }
    }
}

/// The least certain of the recorded sources; `Default` when there are none.
fn worst_source(recorded: &[String]) -> PriceSource {
    recorded
        .iter()
        .map(|name| source_named(name))
        .max_by_key(|source| uncertainty(*source))
        .unwrap_or(PriceSource::Default)
}

/// A source as `PricedCall::provenance` records it (its `Debug` name). An unknown name is treated
/// as no price record at all.
fn source_named(name: &str) -> PriceSource {
    match name {
        "Exact" => PriceSource::Exact,
        "Family" => PriceSource::Family,
        "Static" => PriceSource::Static,
        "Ratio" => PriceSource::Ratio,
        _ => PriceSource::Default,
    }
}

/// How far a source is from a looked-up price, in the order the engine falls through them.
fn uncertainty(source: PriceSource) -> u8 {
    match source {
        PriceSource::Exact => 0,
        PriceSource::Family => 1,
        PriceSource::Static => 2,
        PriceSource::Ratio => 3,
        PriceSource::Default => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(run: Option<&str>, cursor: Option<&str>, limit: Option<&str>) -> UsageParams {
        UsageParams {
            run: run.map(str::to_string),
            cursor: cursor.map(str::to_string),
            limit: limit.map(str::to_string),
        }
    }

    fn code(result: Result<RunQuery, ApiError>) -> &'static str {
        result.expect_err("expected a rejection").1
    }

    #[test]
    fn a_run_becomes_the_range_of_its_tags() {
        let query = RunQuery::parse(params(Some("run-7"), None, None)).unwrap();
        assert_eq!(
            query,
            RunQuery {
                from: "run-7:".into(),
                until: "run-7;".into(),
                cursor: None,
                limit: DEFAULT_LIMIT,
            }
        );
    }

    #[test]
    fn the_range_holds_every_tag_of_the_run_and_none_of_another() {
        let query = RunQuery::parse(params(Some("r1"), None, None)).unwrap();
        let in_range = |tag: &str| {
            tag.as_bytes() >= query.from.as_bytes() && tag.as_bytes() < query.until.as_bytes()
        };
        for tag in ["r1:a", "r1:z:9", "r1:-", "r1:~"] {
            assert!(in_range(tag), "{tag}");
        }
        for tag in ["r1", "r10:a", "r1a:x", "r1;", "r2:a"] {
            assert!(!in_range(tag), "{tag}");
        }
    }

    #[test]
    fn like_and_sql_metacharacters_are_not_a_run() {
        for run in ["run%", "run_'", "a b", "", "r;1", "r\\1"] {
            assert_eq!(
                code(RunQuery::parse(params(Some(run), None, None))),
                "invalid_run",
                "{run}"
            );
        }
        assert_eq!(
            code(RunQuery::parse(params(None, None, None))),
            "invalid_run"
        );
    }

    #[test]
    fn a_run_at_the_length_limit_is_accepted_and_one_past_it_is_not() {
        let at_limit = "r".repeat(MAX_RUN_LEN);
        assert!(RunQuery::parse(params(Some(&at_limit), None, None)).is_ok());
        let past = "r".repeat(MAX_RUN_LEN + 1);
        assert_eq!(
            code(RunQuery::parse(params(Some(&past), None, None))),
            "invalid_run"
        );
    }

    #[test]
    fn limit_must_be_in_range() {
        for bad in ["0", "1001", "-1", "ten"] {
            assert_eq!(
                code(RunQuery::parse(params(Some("r"), None, Some(bad)))),
                "invalid_limit",
                "{bad}"
            );
        }
        let query = RunQuery::parse(params(Some("r"), None, Some("1000"))).unwrap();
        assert_eq!(query.limit, 1000);
    }

    #[test]
    fn a_cursor_outside_the_tag_alphabet_is_refused() {
        assert_eq!(
            code(RunQuery::parse(params(Some("r"), Some("r:1'"), None))),
            "invalid_cursor"
        );
        let query = RunQuery::parse(params(Some("r"), Some("r:1"), None)).unwrap();
        assert_eq!(query.cursor.as_deref(), Some("r:1"));
    }

    #[test]
    fn the_worst_source_is_the_least_certain_one() {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(worst_source(&names(&["Exact"])), PriceSource::Exact);
        assert_eq!(
            worst_source(&names(&["Exact", "Static", "Family"])),
            PriceSource::Static
        );
        assert_eq!(
            worst_source(&names(&["Ratio", "Exact"])),
            PriceSource::Ratio
        );
        assert_eq!(
            worst_source(&names(&["Exact", "Default"])),
            PriceSource::Default
        );
    }

    #[test]
    fn no_or_unknown_price_records_count_as_default() {
        assert_eq!(worst_source(&[]), PriceSource::Default);
        assert_eq!(
            worst_source(&["Exact".to_string(), "Unheard".to_string()]),
            PriceSource::Default
        );
    }

    #[test]
    fn an_attempt_reports_its_models_sorted_and_its_worst_source_by_name() {
        let usage = AttemptRow {
            attempt: "r:t:1".into(),
            calls: 3,
            failed_calls: 1,
            models: vec!["zeta".into(), "flint".into()],
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 2,
            cache_creation_tokens: 1,
            cost_usd: 0.0,
            price_sources: vec!["Exact".into(), "Ratio".into()],
            estimated_calls: 1,
        }
        .into_usage();
        assert_eq!(usage.models, vec!["flint".to_string(), "zeta".to_string()]);
        assert_eq!(usage.worst_price_source, "Ratio");
    }
}
