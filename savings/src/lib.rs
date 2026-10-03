//! The token-savings ledger: what each optimization layer removed, and what that was worth.
//!
//! One row per (call, layer). Several layers act on the same request — payload compression and the
//! brevity directive both touch one chat completion — so the fact table is per-layer rather than
//! per-call, and the dashboard's category breakdown is a `GROUP BY layer` instead of a JSONB bag.
//!
//! # Why this is a crate rather than a module inside a writer
//!
//! Four crates write to this table (`llm-router`, `react-agent`, `orchestrator`, `server`) and the
//! server's observability layer reads it. Hosting the vocabulary inside any one writer would force
//! either a dependency between siblings — `orchestrator` on `llm-router`, which inverts their
//! relationship — or four re-spellings of the layer names as string literals. The names are
//! `CHECK`-constrained in `oss/migrations/0052_token_savings.sql`, so a drifting literal is a
//! runtime insert failure rather than a compile error. [`Layer`] and [`Program`] are here so that
//! vocabulary has exactly one Rust spelling.
//!
//! # What this crate deliberately does not hold
//!
//! Conversion and pricing. Turning bytes into tokens needs the calling layer's own context — the
//! llm-router calibrates against the provider's reported usage for that very request, while the
//! orchestrator-side layers have no provider response in scope and are calibrated at read time.
//! Each writer owns its own derivation and hands this crate a finished row.

#![forbid(unsafe_code)]

use sqlx::PgPool;
use uuid::Uuid;

/// Which optimization layer saved this. The string forms are `CHECK`-constrained by migration
/// 0052; changing one changes a queryable value, so they are spelled out rather than derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// IP-1 — structural compression of tool results at the llm-router egress seam.
    CompressPayload,
    /// IP-3 — the same compression applied to tool results as the ReAct loop stores them.
    CompressToolResult,
    /// IP-4 — compression applied to session history as it is read, before selection.
    CompressHistory,
    /// The budget effect of PACMS/TopK/LastK against the legacy 20-message verbatim window.
    /// Named for the budget, not the selector: every strategy fills the same budget, so the
    /// saving belongs to the tier rather than to the selection algorithm.
    ContextSelection,
    /// IP-2 — factor-derived, never measured. The counterfactual output does not exist.
    Brevity,
    /// Ponytail's decision ladder — factor-derived, for the same reason.
    MinimalCode,
    /// Instruction-file comment stripping in the coding agent.
    PromptComments,
}

/// How a layer is named to users. Several layers roll up to one program — the dashboard says
/// "Caveman saved this much" while the engineering view keeps the four layers underneath it.
///
/// Denormalized onto every row on purpose: the mapping is a product decision about naming, not a
/// property of the layer, and baking it into a read-time `CASE` would hide that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    Caveman,
    /// Labelled "Context budgeting" in the UI — see [`Layer::ContextSelection`].
    Pacms,
    Ponytail,
    PromptComments,
}

/// How the saving was arrived at. The distinction is the whole basis on which a figure can be
/// audited, so it is a column rather than a convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Both sides of the subtraction were held in memory; the saving is a byte count.
    MeasuredBytes,
    /// Both sides were counted in tokens directly, without a byte intermediate.
    MeasuredTokens,
}

impl Layer {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::CompressPayload => "compress_payload",
            Self::CompressToolResult => "compress_tool_result",
            Self::CompressHistory => "compress_history",
            Self::ContextSelection => "context_selection",
            Self::Brevity => "brevity",
            Self::MinimalCode => "minimal_code",
            Self::PromptComments => "prompt_comments",
        }
    }

    /// The program this layer rolls up to. Fixed here rather than at each call site so two writers
    /// cannot file the same layer under different programs.
    pub fn program(self) -> Program {
        match self {
            Self::CompressPayload
            | Self::CompressToolResult
            | Self::CompressHistory
            | Self::Brevity => Program::Caveman,
            Self::ContextSelection => Program::Pacms,
            Self::MinimalCode => Program::Ponytail,
            Self::PromptComments => Program::PromptComments,
        }
    }
}

impl Program {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Caveman => "caveman",
            Self::Pacms => "pacms",
            Self::Ponytail => "ponytail",
            Self::PromptComments => "prompt_comments",
        }
    }
}

impl Method {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::MeasuredBytes => "measured_bytes",
            Self::MeasuredTokens => "measured_tokens",
        }
    }
}

/// One ledger row, mirroring `token_savings`.
///
/// Token and cost fields are **signed**. Context selection can legitimately come out negative — a
/// `high` budget tier can admit more than the 20-message baseline would have — and clamping that
/// to zero would turn a measurement into a marketing number.
#[derive(Debug, Clone, PartialEq)]
pub struct SavingsRow {
    pub user_id: Uuid,
    pub agent_id: Option<Uuid>,
    /// W3C traceparent trace-id. Joins to `token_usage.session_id` and `trace_usage.trace_id`.
    pub flow_id: Option<String>,
    /// A2A contextId. `None` from writers that do not hold one — the read path resolves it
    /// through `flow_id` rather than spending a lookup per call on the hot path.
    pub session_id: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub layer: Layer,
    pub bytes_before: Option<i64>,
    pub bytes_after: Option<i64>,
    pub saved_input_tokens: i64,
    pub saved_output_tokens: i64,
    pub saved_cost_usd: f64,
    pub method: Method,
    /// True when chars-per-token fell back to a shared divisor instead of being calibrated against
    /// real reported usage. The headline percentage divides this numerator by a provider-reported
    /// denominator, so the error does not cancel — the read path reports what fraction of a window
    /// was calibrated.
    pub token_estimated: bool,
    /// The user's PACMS budget tier at call time. Only meaningful on
    /// [`Layer::ContextSelection`] rows, where the tier is what moves the number.
    pub context_tier: Option<String>,
}

/// Insert one row.
///
/// Best-effort by design, like the `token_usage` write it rides alongside: a savings row that
/// fails to land must never surface as an error on a request that already succeeded. Accounting is
/// downstream of the product working, not a precondition for it.
pub async fn insert(db: &PgPool, row: &SavingsRow) {
    let result = sqlx::query(
        r#"INSERT INTO token_savings
               (user_id, agent_id, flow_id, session_id, provider, model, layer, program,
                bytes_before, bytes_after, saved_input_tokens, saved_output_tokens,
                saved_cost_usd, method, token_estimated, context_tier)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)"#,
    )
    .bind(row.user_id)
    .bind(row.agent_id)
    .bind(&row.flow_id)
    .bind(&row.session_id)
    .bind(&row.provider)
    .bind(&row.model)
    .bind(row.layer.as_label())
    .bind(row.layer.program().as_label())
    .bind(row.bytes_before)
    .bind(row.bytes_after)
    .bind(row.saved_input_tokens)
    .bind(row.saved_output_tokens)
    .bind(row.saved_cost_usd)
    .bind(row.method.as_label())
    .bind(row.token_estimated)
    .bind(&row.context_tier)
    .execute(db)
    .await;

    if let Err(e) = result {
        tracing::warn!(
            error = %e,
            layer = row.layer.as_label(),
            "token_savings write failed (swallowed)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_LAYERS: [Layer; 7] = [
        Layer::CompressPayload,
        Layer::CompressToolResult,
        Layer::CompressHistory,
        Layer::ContextSelection,
        Layer::Brevity,
        Layer::MinimalCode,
        Layer::PromptComments,
    ];

    /// The labels are `CHECK`-constrained by migration 0052, so a drifting one is a runtime insert
    /// failure rather than a compile error. This is the guard that turns it back into a test
    /// failure.
    #[test]
    fn layer_labels_match_the_migrations_check_constraint() {
        let expected = [
            "compress_payload",
            "compress_tool_result",
            "compress_history",
            "context_selection",
            "brevity",
            "minimal_code",
            "prompt_comments",
        ];
        let actual: Vec<&str> = ALL_LAYERS.iter().map(|l| l.as_label()).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn program_labels_match_the_dashboards_category_rows() {
        for (program, label) in [
            (Program::Caveman, "caveman"),
            (Program::Pacms, "pacms"),
            (Program::Ponytail, "ponytail"),
            (Program::PromptComments, "prompt_comments"),
        ] {
            assert_eq!(program.as_label(), label);
        }
    }

    #[test]
    fn method_labels_match_the_migrations_check_constraint() {
        assert_eq!(Method::MeasuredBytes.as_label(), "measured_bytes");
        assert_eq!(Method::MeasuredTokens.as_label(), "measured_tokens");
    }

    /// Every compression layer rolls up to Caveman, including brevity — they are four injection
    /// points of one program, and the dashboard names the program.
    #[test]
    fn every_caveman_layer_rolls_up_to_caveman() {
        for layer in [
            Layer::CompressPayload,
            Layer::CompressToolResult,
            Layer::CompressHistory,
            Layer::Brevity,
        ] {
            assert_eq!(layer.program(), Program::Caveman, "{layer:?}");
        }
    }

    #[test]
    fn context_selection_is_its_own_program_not_a_caveman_layer() {
        // It measures the budget tier, not compression — rolling it under Caveman would credit
        // the wrong feature for the saving.
        assert_eq!(Layer::ContextSelection.program(), Program::Pacms);
        assert_eq!(Layer::MinimalCode.program(), Program::Ponytail);
        assert_eq!(Layer::PromptComments.program(), Program::PromptComments);
    }

    #[test]
    fn every_layer_has_a_distinct_label() {
        let mut labels: Vec<&str> = ALL_LAYERS.iter().map(|l| l.as_label()).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "two layers share a label");
    }
}
