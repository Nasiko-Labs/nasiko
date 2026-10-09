//! OpenAI provider — the simplest spoke: the agent already speaks OpenAI, so this is
//! ≈passthrough. We override the model with the resolved one (C4), apply the param
//! precedence (resolved wins when set, else the request's), force non-streaming on the
//! non-stream path, call the provider, and report the bare resolved model.
//!
//! This spoke also serves every custom (DB-registered) endpoint, including Azure
//! OpenAI. The bodies are identical across those; only the envelope — URL layout and
//! credential header — differs, and that lives in [`ProviderDialect`] rather than in a
//! forked copy of this file.

use std::collections::{BTreeMap, HashSet};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use super::dialect::ProviderDialect;
use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError};
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse, Message, ToolDef,
};
use crate::resolver::ResolvedConfig;

pub struct OpenAiProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://api.openai.com/v1` (overridable for tests).
    base: String,
    /// Envelope this endpoint speaks — URL layout + credential header.
    dialect: ProviderDialect,
    /// Tool-array cap this endpoint enforces; `0` = uncapped. See [`fit_tools`].
    max_tools: usize,
}

impl OpenAiProvider {
    /// A plain OpenAI-compatible endpoint at `base`.
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self::with_dialect(http, base, ProviderDialect::OpenAi)
    }

    /// An OpenAI-shaped endpoint at `base` reached through `dialect`.
    pub fn with_dialect(http: reqwest::Client, base: String, dialect: ProviderDialect) -> Self {
        Self {
            http,
            base,
            dialect,
            // Uncapped unless the composition root says otherwise, so a provider built
            // anywhere else keeps forwarding exactly what the caller sent.
            max_tools: 0,
        }
    }

    /// Cap the tool array this endpoint will accept (`0` = uncapped).
    pub fn with_max_tools(mut self, max_tools: usize) -> Self {
        self.max_tools = max_tools;
        self
    }

    /// Fit the outbound tool array into this endpoint's cap, logging what was trimmed.
    fn apply_tool_budget(&self, out: &mut ChatRequest, model: &str) {
        let Some(tools) = out.tools.take() else {
            return;
        };
        let query = latest_user_text(&out.messages);
        let (kept, dropped) = fit_tools(tools, self.max_tools, &query);
        if !dropped.is_empty() {
            tracing::warn!(
                target: "nasiko::llm_router::provider",
                model, cap = self.max_tools, kept = kept.len(), dropped = dropped.len(),
                by_server = %dropped_by_server(&dropped),
                "tool array exceeds this endpoint's cap; forwarding the most relevant \
                 subset — a tool trimmed here cannot be called this turn"
            );
        }
        out.tools = Some(kept);
    }

    /// Map a non-2xx provider response to a [`ProviderError`]. 429 and 5xx are
    /// retryable (transient); other 4xx are request-shape errors and are not.
    fn status_error(status: reqwest::StatusCode, body: String) -> ProviderError {
        ProviderError::Status {
            status: status.as_u16(),
            message: body,
            retryable: status.as_u16() == 429 || status.is_server_error(),
        }
    }
}

/// The OpenAI `{"error":{"param":…,"code":"unsupported_…"}}` rejection shape: the name
/// of a parameter the model does not accept, or `None` for any other 400.
fn openai_droppable_param(body: &str) -> Option<String> {
    let body: serde_json::Value = serde_json::from_str(body).ok()?;
    let error = body.get("error")?;
    let code = error
        .get("code")
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    // Only these codes mean "this param/value isn't accepted here" — safe to drop.
    if !matches!(code, "unsupported_value" | "unsupported_parameter") {
        return None;
    }
    error
        .get("param")
        .and_then(|p| p.as_str())
        .map(str::to_string)
}

/// The rejections a drop cannot fix: the model names a remedy, so the retry has to *set*
/// a parameter rather than strip one. Three shapes, each gated on `error.param` **and**
/// the remedy wording OpenAI itself prints, so none can misfire on another 400 that
/// merely mentions the field:
///
/// 1. `{"message":"Function tools with reasoning_effort are not supported for gpt-5.6 …
///    set reasoning_effort to 'none'.","param":"reasoning_effort","code":null}` —
///    [`openai_droppable_param`] cannot see this one (the `code` is null rather than an
///    `unsupported_*`) and dropping would not help anyway: we never send
///    `reasoning_effort`, so what the model rejects is its own default. Send the remedy.
/// 2. `{"message":"Unsupported value: 'reasoning_effort' does not support 'none' with this
///    model. Supported values are: 'low', 'medium', 'high', and 'xhigh'.",…,"code":
///    "unsupported_value"}` — a model that still refuses function tools at its default
///    effort but has dropped `'none'`, so it rejects the repair (1) just applied.
///    Dropping the param here would only reinstate the default that failed in (1), which
///    is why this shape must not fall through to the droppable seam.
/// 3. `{"message":"max_tokens is too large: 64000. This model supports at most 16384
///    completion tokens, whereas you provided 64000.",…,"code":"invalid_value"}` — the
///    caller asked for more output than the model allows. Clamp to the stated ceiling;
///    dropping the cap would discard the caller's intent entirely.
fn openai_repairable_param(body: &str) -> Option<(String, Value)> {
    let body: Value = serde_json::from_str(body).ok()?;
    let error = body.get("error")?;
    let param = error.get("param")?.as_str()?;
    let message = error.get("message")?.as_str()?;
    match param {
        "reasoning_effort" if message.contains("reasoning_effort to 'none'") => {
            Some((param.to_string(), json!("none")))
        }
        "reasoning_effort" => Some((param.to_string(), json!(cheapest_listed_value(message)?))),
        "max_tokens" | "max_completion_tokens" => {
            Some((param.to_string(), json!(stated_token_ceiling(message)?)))
        }
        _ => None,
    }
}

/// The first value OpenAI lists in `Supported values are: 'low', 'medium', 'high', and
/// 'xhigh'.` — it enumerates them cheapest first, and the cheapest accepted effort is the
/// closest stand-in for the `'none'` the model refused.
fn cheapest_listed_value(message: &str) -> Option<String> {
    let listed = message.split_once("Supported values are:")?.1;
    let (value, _) = listed.split_once('\'')?.1.split_once('\'')?;
    Some(value.to_string())
}

/// The ceiling OpenAI states in `This model supports at most 16384 completion tokens`.
/// Digit grouping is tolerated so a `16,384` never parses as `16`.
fn stated_token_ceiling(message: &str) -> Option<i64> {
    let digits: String = message
        .split_once("supports at most")?
        .1
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Prefix every MCP-sourced tool carries (`mcp__<server>__<tool>`). A tool without it
/// is the caller's own — the client's built-ins — and is never the thing to trim: it is
/// what the agent actually works with, and there are only ever a handful.
const MCP_PREFIX: &str = "mcp__";

/// BM25 term-frequency saturation and length-normalization constants (the standard
/// defaults; this ranking only has to order tools sensibly, not be tuned).
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// Reduce `tools` to `cap` entries, returning the kept set (in the caller's original
/// order) and the names dropped.
///
/// OpenAI's `/v1/chat/completions` rejects more than 128 tools outright, so a client
/// with several MCP servers attached cannot talk to *any* OpenAI model without this —
/// the alternative is not a degraded answer but a 400 on every turn. Two rules, in
/// order: keep the caller's own tools, then spend what is left of the budget on the MCP
/// tools whose name and description best match the current turn. Dropping is real
/// capability loss, which is why it is ranked rather than arbitrary and logged loudly.
fn fit_tools(tools: Vec<ToolDef>, cap: usize, query: &str) -> (Vec<ToolDef>, Vec<String>) {
    if cap == 0 || tools.len() <= cap {
        return (tools, Vec::new());
    }
    let (core, optional): (Vec<usize>, Vec<usize>) =
        (0..tools.len()).partition(|&i| !tools[i].function.name.starts_with(MCP_PREFIX));

    // The caller's own tools come first, and only a pathological client sends more of
    // those than the cap — truncate rather than panic if one does.
    let mut keep: HashSet<usize> = core.into_iter().take(cap).collect();
    let budget = cap.saturating_sub(keep.len());
    if budget > 0 {
        keep.extend(
            rank_by_relevance(&tools, &optional, query)
                .into_iter()
                .take(budget),
        );
    }

    let mut dropped = Vec::new();
    let kept = tools
        .into_iter()
        .enumerate()
        .filter_map(|(i, tool)| {
            if keep.contains(&i) {
                Some(tool)
            } else {
                dropped.push(tool.function.name);
                None
            }
        })
        .collect();
    (kept, dropped)
}

/// Order `candidates` (indices into `tools`) by BM25 relevance to `query`, best first.
/// An empty query carries no signal, so the caller's order stands.
fn rank_by_relevance(tools: &[ToolDef], candidates: &[usize], query: &str) -> Vec<usize> {
    let terms = tokenize(query);
    if terms.is_empty() || candidates.is_empty() {
        return candidates.to_vec();
    }
    let docs: Vec<Vec<String>> = candidates
        .iter()
        .map(|&i| tokenize(&tool_text(&tools[i])))
        .collect();
    let n = docs.len() as f64;
    let avg_len = (docs.iter().map(Vec::len).sum::<usize>() as f64 / n).max(1.0);
    // Document frequency per query term, computed once rather than per document.
    let dfs: Vec<f64> = terms
        .iter()
        .map(|t| docs.iter().filter(|d| d.contains(t)).count() as f64)
        .collect();

    let mut scored: Vec<(usize, f64)> = candidates
        .iter()
        .copied()
        .zip(&docs)
        .map(|(idx, doc)| (idx, bm25(&terms, &dfs, doc, n, avg_len)))
        .collect();
    // `sort_by` is stable, so equally irrelevant tools keep the caller's order.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.into_iter().map(|(i, _)| i).collect()
}

fn bm25(terms: &[String], dfs: &[f64], doc: &[String], n: f64, avg_len: f64) -> f64 {
    let len = doc.len() as f64;
    terms
        .iter()
        .zip(dfs)
        .map(|(term, &df)| {
            let tf = doc.iter().filter(|w| *w == term).count() as f64;
            if tf == 0.0 {
                return 0.0;
            }
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            idf * (tf * (BM25_K1 + 1.0)) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * len / avg_len))
        })
        .sum()
}

/// The text a tool is matched on: its name (which carries the server and verb) plus its
/// description. The argument schema is deliberately excluded — it is mostly type noise.
fn tool_text(tool: &ToolDef) -> String {
    match &tool.function.description {
        Some(d) => format!("{} {d}", tool.function.name),
        None => tool.function.name.clone(),
    }
}

/// Lowercased alphanumeric runs. Splitting on non-alphanumerics breaks
/// `mcp__linear__list_issues` into its parts; single characters carry no signal.
fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 1)
        .map(str::to_lowercase)
        .collect()
}

/// The latest user turn — what the tools for *this* request should be relevant to.
fn latest_user_text(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(Message::text)
        .unwrap_or_default()
}

/// `mcp__linear__list_issues` → `linear`, so the warning says which integrations lost
/// tools rather than printing a hundred names.
fn server_of(name: &str) -> &str {
    name.strip_prefix(MCP_PREFIX)
        .and_then(|rest| rest.split("__").next())
        .unwrap_or("caller")
}

fn dropped_by_server(dropped: &[String]) -> String {
    let mut counts: Vec<(&str, usize)> = dropped
        .iter()
        .fold(BTreeMap::new(), |mut acc, name| {
            *acc.entry(server_of(name)).or_insert(0) += 1;
            acc
        })
        .into_iter()
        .collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    counts
        .iter()
        .map(|(server, n)| format!("{server}={n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait]
impl ProviderClient for OpenAiProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone()); // C4: resolved model is authoritative
        out.temperature = cfg.temperature.or(req.temperature); // resolved wins when set
        // OpenAI deprecated `max_tokens`; newer models reject it. Emit the current
        // `max_completion_tokens` param instead (via passthrough; the named field is
        // cleared so it doesn't also serialize).
        out.max_tokens = None;
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.extra
                .insert("max_completion_tokens".to_string(), json!(mt));
        }
        out.stream = Some(false);
        self.apply_tool_budget(&mut out, &cfg.model);

        let resp = self
            .dialect
            .authorize(
                self.http
                    .post(self.dialect.chat_url(&self.base, &cfg.model)),
                &cfg.api_key,
            )
            .json(&out)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::status_error(status, body));
        }

        let mut parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        parsed.model = cfg.model.clone(); // report the bare resolved model id
        Ok(parsed)
    }

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone());
        out.temperature = cfg.temperature.or(req.temperature);
        // See `chat` — OpenAI wants `max_completion_tokens`, not the deprecated `max_tokens`.
        out.max_tokens = None;
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.extra
                .insert("max_completion_tokens".to_string(), json!(mt));
        }
        out.stream = Some(true);
        // Ask OpenAI to emit a final usage chunk (off by default when streaming).
        out.extra.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
        self.apply_tool_budget(&mut out, &cfg.model);

        let resp = self
            .dialect
            .authorize(
                self.http
                    .post(self.dialect.chat_url(&self.base, &cfg.model)),
                &cfg.api_key,
            )
            .json(&out)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::status_error(status, body));
        }

        // OpenAI chunks are already in IR shape; parse each, normalize the model.
        let model = cfg.model.clone();
        let data = sse_data_stream(resp.bytes_stream());
        let stream = async_stream::stream! {
            futures::pin_mut!(data);
            while let Some(item) = data.next().await {
                match item {
                    Err(e) => { yield Err(e); return; }
                    Ok(payload) => {
                        if payload.trim() == "[DONE]" {
                            break;
                        }
                        // Skip unparseable lines (comments/keep-alives) rather than failing.
                        if let Ok(mut chunk) = serde_json::from_str::<ChatChunk>(&payload) {
                            chunk.model = model.clone();
                            yield Ok(chunk);
                        }
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }

    async fn embeddings(
        &self,
        req: &EmbeddingsRequest,
        cfg: &ResolvedConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone());

        let resp = self
            .dialect
            .authorize(
                self.http
                    .post(self.dialect.embeddings_url(&self.base, &cfg.model)),
                &cfg.api_key,
            )
            .json(&out)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::status_error(status, body));
        }

        let mut parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        parsed.model = cfg.model.clone();
        Ok(parsed)
    }

    /// OpenAI reports a model/parameter mismatch as a 400 whose body names the offending
    /// field: `{"error":{"param":"temperature","code":"unsupported_value",...}}`. When
    /// that's the shape, return the param so the executor can drop it and retry the same
    /// model (dropping a param makes OpenAI apply its default — e.g. temperature → 1).
    /// This is general: any param OpenAI rejects this way is handled without special-casing.
    ///
    /// A dialect that reports the same class of failure differently (Azure names the
    /// field in a bare message) gets a second look through
    /// [`ProviderDialect::extra_droppable_param`].
    fn droppable_param(&self, err: &ProviderError) -> Option<String> {
        let ProviderError::Status {
            status, message, ..
        } = err
        else {
            return None;
        };
        if *status != 400 {
            return None;
        }
        // A rejection the model itself tells us how to repair is never also a drop:
        // stripping the param would reinstate the very default it refused. Checked first
        // so shape (2) in [`openai_repairable_param`] — which does carry an
        // `unsupported_value` code — reaches the repair seam instead of being swallowed
        // here as an (unfixable) drop.
        if openai_repairable_param(message).is_some() {
            return None;
        }
        openai_droppable_param(message).or_else(|| self.dialect.extra_droppable_param(message))
    }

    /// The rejection classes a drop cannot fix — a reasoning model refusing function tools
    /// at its default effort, and an output cap above the model's ceiling. See
    /// [`openai_repairable_param`].
    fn repairable_param(&self, err: &ProviderError) -> Option<(String, Value)> {
        let ProviderError::Status {
            status, message, ..
        } = err
        else {
            return None;
        };
        if *status != 400 {
            return None;
        }
        openai_repairable_param(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolved(model: &str, temperature: Option<f64>) -> ResolvedConfig {
        ResolvedConfig {
            compress_enabled: false,
            provider: "openai".into(),
            model: model.into(),
            litellm_model: format!("openai/{model}"),
            api_key: "sk-test".into(),
            fallback_models: vec![],
            temperature,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
        }
    }

    #[test]
    fn droppable_param_extracts_offending_field_from_openai_400() {
        let provider = OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        // The exact error shape gpt-5.5 returns for a non-default temperature.
        let unsupported = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Unsupported value: 'temperature' does not support 0.1 with this model. Only the default (1) value is supported.",
                    "type": "invalid_request_error",
                    "param": "temperature",
                    "code": "unsupported_value"
                }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(
            provider.droppable_param(&unsupported).as_deref(),
            Some("temperature")
        );

        // A different code (bad key, model not found, quota) is not a droppable param.
        let other_400 = ProviderError::Status {
            status: 400,
            message: json!({ "error": { "code": "invalid_api_key", "param": "temperature" } })
                .to_string(),
            retryable: false,
        };
        assert_eq!(provider.droppable_param(&other_400), None);

        // 5xx / transport / unparseable bodies are never a droppable param.
        assert_eq!(
            provider.droppable_param(&ProviderError::Status {
                status: 500,
                message: "boom".into(),
                retryable: true
            }),
            None
        );
        assert_eq!(
            provider.droppable_param(&ProviderError::Transport("timeout".into())),
            None
        );
        assert_eq!(
            provider.droppable_param(&ProviderError::Status {
                status: 400,
                message: "not json".into(),
                retryable: false
            }),
            None
        );
    }

    /// The verbatim body a gpt-5.x reasoning model returns when a request carries
    /// function tools — every Claude Code turn, which always sends its tool set.
    fn reasoning_effort_rejection() -> ProviderError {
        ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Function tools with reasoning_effort are not supported for gpt-5.6 in /v1/chat/completions. To use function tools, use /v1/responses or set reasoning_effort to 'none'.",
                    "type": "invalid_request_error",
                    "param": "reasoning_effort",
                    "code": Value::Null
                }
            })
            .to_string(),
            retryable: false,
        }
    }

    /// The follow-on rejection from a model that refuses function tools at its default
    /// effort *and* has dropped `'none'` — i.e. it rejects the repair we just applied.
    fn effort_none_unsupported_rejection() -> ProviderError {
        ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Unsupported value: 'reasoning_effort' does not support 'none' with this model. Supported values are: 'low', 'medium', 'high', and 'xhigh'.",
                    "type": "invalid_request_error",
                    "param": "reasoning_effort",
                    "code": "unsupported_value"
                }
            })
            .to_string(),
            retryable: false,
        }
    }

    /// The rejection a model returns when the caller's output cap exceeds its ceiling —
    /// every Claude Code turn, which asks for 64000 regardless of the resolved model.
    fn max_tokens_too_large_rejection() -> ProviderError {
        ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "max_tokens is too large: 64000. This model supports at most 16384 completion tokens, whereas you provided 64000.",
                    "type": "invalid_request_error",
                    "param": "max_tokens",
                    "code": "invalid_value"
                }
            })
            .to_string(),
            retryable: false,
        }
    }

    #[test]
    fn repairable_param_recognizes_the_reasoning_effort_rejection() {
        let provider = OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        assert_eq!(
            provider.repairable_param(&reasoning_effort_rejection()),
            Some(("reasoning_effort".to_string(), json!("none")))
        );
    }

    #[test]
    fn repairable_param_takes_the_cheapest_effort_a_model_without_none_lists() {
        let provider = OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        assert_eq!(
            provider.repairable_param(&effort_none_unsupported_rejection()),
            Some(("reasoning_effort".to_string(), json!("low")))
        );
    }

    #[test]
    fn repairable_param_clamps_an_output_cap_to_the_stated_ceiling() {
        let provider = OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        assert_eq!(
            provider.repairable_param(&max_tokens_too_large_rejection()),
            Some(("max_tokens".to_string(), json!(16384)))
        );

        // Digit grouping must not truncate the ceiling to 16.
        let grouped = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "max_tokens is too large: 64000. This model supports at most 16,384 completion tokens.",
                    "param": "max_completion_tokens",
                    "code": "invalid_value"
                }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(
            provider.repairable_param(&grouped),
            Some(("max_completion_tokens".to_string(), json!(16384)))
        );

        // The right param with no stated ceiling names no remedy, so it is not repairable.
        let no_ceiling = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "max_tokens must be a positive integer.",
                    "param": "max_tokens",
                    "code": "invalid_value"
                }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(provider.repairable_param(&no_ceiling), None);
    }

    #[test]
    fn the_two_recovery_seams_never_both_claim_an_error() {
        let provider = OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        // A drop cannot fix the reasoning rejection (we never sent the param), so the
        // droppable seam must not claim it — its `code` is null, not `unsupported_*`.
        assert_eq!(
            provider.droppable_param(&reasoning_effort_rejection()),
            None
        );

        // These two DO carry codes the droppable seam reads, so the repair must win: a
        // drop here reinstates the default effort that failed one call earlier, or throws
        // away the caller's output cap instead of clamping it.
        assert_eq!(
            provider.droppable_param(&effort_none_unsupported_rejection()),
            None
        );
        assert_eq!(
            provider.droppable_param(&max_tokens_too_large_rejection()),
            None
        );

        // And the repair seam must not claim a plain droppable-param rejection.
        let unsupported = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Unsupported value: 'temperature' does not support 0.1 with this model.",
                    "param": "temperature",
                    "code": "unsupported_value"
                }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(provider.repairable_param(&unsupported), None);
    }

    #[test]
    fn repairable_param_ignores_anything_but_this_exact_rejection() {
        let provider = OpenAiProvider::new(reqwest::Client::new(), "http://x".into());

        // Right param, different complaint — no remedy to apply, so not repairable.
        let other_complaint = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Invalid value for 'reasoning_effort': expected one of low, medium, high.",
                    "param": "reasoning_effort",
                    "code": Value::Null
                }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(provider.repairable_param(&other_complaint), None);

        // The remedy text alone, under a different param, is not enough either.
        let other_param = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "set reasoning_effort to 'none'",
                    "param": "tools",
                    "code": Value::Null
                }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(provider.repairable_param(&other_param), None);

        // 5xx / transport / unparseable bodies are never repairable.
        assert_eq!(
            provider.repairable_param(&ProviderError::Status {
                status: 500,
                message: "boom".into(),
                retryable: true
            }),
            None
        );
        assert_eq!(
            provider.repairable_param(&ProviderError::Transport("timeout".into())),
            None
        );
        assert_eq!(
            provider.repairable_param(&ProviderError::Status {
                status: 400,
                message: "not json".into(),
                retryable: false
            }),
            None
        );
    }

    fn tool(name: &str, description: &str) -> ToolDef {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": { "type": "object", "properties": {} }
            }
        }))
        .expect("valid tool")
    }

    fn names(tools: &[ToolDef]) -> Vec<&str> {
        tools.iter().map(|t| t.function.name.as_str()).collect()
    }

    #[test]
    fn a_tool_array_within_the_cap_is_forwarded_untouched() {
        let tools = vec![
            tool("Read", "read a file"),
            tool("mcp__linear__x", "issues"),
        ];
        let (kept, dropped) = fit_tools(tools.clone(), 128, "anything");
        assert_eq!(names(&kept), names(&tools));
        assert!(dropped.is_empty());

        // cap 0 means the endpoint has no limit — never trim, however many there are.
        let (kept, dropped) = fit_tools(tools.clone(), 0, "anything");
        assert_eq!(kept.len(), 2);
        assert!(dropped.is_empty());
    }

    #[test]
    fn the_callers_own_tools_survive_and_mcp_tools_are_kept_by_relevance() {
        // One built-in plus three MCP tools, budget for two of them. The built-in is
        // never a candidate for trimming, so exactly one MCP tool must lose.
        let tools = vec![
            tool("Read", "read a file from disk"),
            tool(
                "mcp__figma__get_screenshot",
                "capture a screenshot of a frame",
            ),
            tool("mcp__linear__list_issues", "list issues in a linear team"),
            tool("mcp__slack__post", "send a chat message to a channel"),
        ];
        let (kept, dropped) = fit_tools(tools, 3, "list the open linear issues for my team");

        assert!(
            names(&kept).contains(&"Read"),
            "built-ins are never trimmed"
        );
        assert!(
            names(&kept).contains(&"mcp__linear__list_issues"),
            "the tool matching the turn must survive: {:?}",
            names(&kept)
        );
        assert_eq!(kept.len(), 3);
        assert_eq!(dropped.len(), 1);
        // Survivors keep the caller's ordering, not the ranking's.
        assert_eq!(names(&kept)[0], "Read");
    }

    #[test]
    fn with_no_query_to_rank_against_the_callers_order_decides() {
        let tools = vec![
            tool("mcp__a__one", "alpha"),
            tool("mcp__b__two", "beta"),
            tool("mcp__c__three", "gamma"),
        ];
        let (kept, dropped) = fit_tools(tools, 2, "");
        assert_eq!(names(&kept), vec!["mcp__a__one", "mcp__b__two"]);
        assert_eq!(dropped, vec!["mcp__c__three".to_string()]);
    }

    #[test]
    fn more_caller_tools_than_the_cap_truncates_rather_than_panicking() {
        let tools = vec![tool("A", "a"), tool("B", "b"), tool("C", "c")];
        let (kept, dropped) = fit_tools(tools, 2, "q");
        assert_eq!(kept.len(), 2);
        assert_eq!(dropped.len(), 1);
    }

    #[test]
    fn the_dropped_summary_counts_per_mcp_server() {
        let dropped = vec![
            "mcp__linear__a".to_string(),
            "mcp__linear__b".to_string(),
            "mcp__figma__c".to_string(),
            "Read".to_string(),
        ];
        assert_eq!(dropped_by_server(&dropped), "linear=2 caller=1 figma=1");
    }

    #[tokio::test]
    async fn an_oversized_tool_array_is_cut_to_the_cap_before_the_call() {
        // The shape that 502s today: a client with several MCP servers attached sends
        // far more tools than /v1/chat/completions accepts.
        let mut server = mockito::Server::new_async().await;
        // The assertion lives in the matcher: a body that isn't exactly 128 tools, led by
        // the caller's own, does not match, and the call then fails instead of passing.
        let m = server
            .mock("POST", "/chat/completions")
            .match_request(|req| {
                let body: Value = serde_json::from_slice(req.body().unwrap()).unwrap();
                let tools = body["tools"].as_array().expect("tools forwarded");
                tools.len() == 128 && tools[0]["function"]["name"] == "Read"
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-5.5",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let mut tools = vec![tool("Read", "read a file")];
        tools.extend((0..241).map(|i| tool(&format!("mcp__srv__t{i}"), "a tool")));
        let req = ChatRequest {
            tools: Some(tools),
            ..serde_json::from_value(json!({
                "messages": [{ "role": "user", "content": "hi" }]
            }))
            .unwrap()
        };

        let provider =
            OpenAiProvider::new(reqwest::Client::new(), server.url()).with_max_tools(128);
        provider
            .chat(&req, &resolved("gpt-5.5", None))
            .await
            .unwrap();

        m.assert_async().await;
    }

    #[tokio::test]
    async fn azure_dialect_addresses_the_deployment_with_an_api_key_header() {
        // The whole point of the dialect: same body, different envelope. Azure puts the
        // deployment in the path, demands `?api-version=`, and takes the credential in
        // `api-key` — `Authorization: Bearer` there means an Entra ID token and fails.
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock(
                "POST",
                "/openai/deployments/prod-gpt4o/chat/completions?api-version=2024-10-21",
            )
            .match_header("api-key", "sk-test")
            .match_header("authorization", mockito::Matcher::Missing)
            .match_body(mockito::Matcher::PartialJson(
                json!({ "model": "prod-gpt4o", "stream": false }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1",
                    "object": "chat.completion",
                    "model": "gpt-4o-2024-08-06",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = OpenAiProvider::with_dialect(
            reqwest::Client::new(),
            server.url(),
            ProviderDialect::AzureOpenAi {
                api_version: "2024-10-21".into(),
            },
        );
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        // The resolved "model" is the Azure deployment name.
        let resp = provider
            .chat(&req, &resolved("prod-gpt4o", None))
            .await
            .unwrap();

        m.assert_async().await;
        assert_eq!(resp.model, "prod-gpt4o");
    }

    #[test]
    fn azure_param_rejection_without_a_param_field_is_still_droppable() {
        // Azure rejects a param its api-version doesn't know with a bare message and no
        // `param` field, so the OpenAI shape alone would miss it and the call would fail
        // instead of retrying without the param.
        let azure = OpenAiProvider::with_dialect(
            reqwest::Client::new(),
            "https://acme.openai.azure.com".into(),
            ProviderDialect::AzureOpenAi {
                api_version: "2023-05-15".into(),
            },
        );
        let err = ProviderError::Status {
            status: 400,
            message: r#"{"error":{"code":"BadRequest","message":"Unrecognized request argument supplied: max_completion_tokens"}}"#.into(),
            retryable: false,
        };
        assert_eq!(
            azure.droppable_param(&err).as_deref(),
            Some("max_completion_tokens")
        );
        // The plain dialect does not read Azure's shape.
        let plain = OpenAiProvider::new(reqwest::Client::new(), "https://x".into());
        assert_eq!(plain.droppable_param(&err), None);
    }

    #[tokio::test]
    async fn chat_overrides_model_applies_precedence_and_reports_bare_model() {
        let mut server = mockito::Server::new_async().await;
        let provider_body = json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            // OpenAI may echo a dated model; we must report the bare resolved one.
            "model": "gpt-4o-mini-2024-07-18",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
        });
        let m = server
            .mock("POST", "/chat/completions")
            // outbound must carry the RESOLVED model (request's "gpt-4o" discarded),
            // the resolved temperature (0.2, not the request's 0.9), and stream:false.
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "gpt-4o-mini", "temperature": 0.2, "stream": false,
                // request's max_tokens must be emitted as max_completion_tokens
                "max_completion_tokens": 256
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(provider_body.to_string())
            .create_async()
            .await;

        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "hi" }],
            "temperature": 0.9,
            "max_tokens": 256
        }))
        .unwrap();
        let resp = provider
            .chat(&req, &resolved("gpt-4o-mini", Some(0.2)))
            .await
            .unwrap();

        m.assert_async().await;
        assert_eq!(resp.model, "gpt-4o-mini"); // bare resolved id, not the echoed dated one
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("hello"));
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(7));
    }

    #[tokio::test]
    async fn embeddings_overrides_model_and_parses() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/embeddings")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "model": "text-embedding-3-small" }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "object": "list",
                    "data": [{ "object": "embedding", "embedding": [0.5, 0.6], "index": 0 }],
                    "model": "text-embedding-3-small-v2",
                    "usage": { "prompt_tokens": 2, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: crate::ir::EmbeddingsRequest =
            serde_json::from_value(json!({ "model": "whatever", "input": "hi" })).unwrap();
        let resp = provider
            .embeddings(&req, &resolved("text-embedding-3-small", None))
            .await
            .unwrap();
        assert_eq!(resp.model, "text-embedding-3-small"); // bare resolved
        assert_eq!(resp.data[0].embedding[0], 0.5);
    }

    #[tokio::test]
    async fn server_error_is_retryable() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(503)
            .with_body("overloaded")
            .create_async()
            .await;
        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = provider
            .chat(&req, &resolved("gpt-4o-mini", None))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Status {
                retryable: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn chat_stream_parses_sse_chunks_and_overrides_model() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
            "data: [DONE]\n\n",
        );
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "stream": true, "stream_options": { "include_usage": true } }),
            ))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o", "stream": true,
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        let stream = provider
            .chat_stream(&req, &resolved("gpt-4o-mini", None))
            .await
            .unwrap();
        let chunks: Vec<ChatChunk> = stream.filter_map(|r| async { r.ok() }).collect().await;

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].model, "gpt-4o-mini"); // normalized to resolved
        assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("Hel"));
        assert_eq!(chunks[1].choices[0].delta.content.as_deref(), Some("lo"));
        assert_eq!(chunks[2].choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(chunks[2].usage.as_ref().unwrap().total_tokens, Some(3));
    }

    #[tokio::test]
    async fn client_error_is_not_retryable() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(400)
            .with_body("bad request")
            .create_async()
            .await;
        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = provider
            .chat(&req, &resolved("gpt-4o-mini", None))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Status {
                retryable: false,
                status: 400,
                ..
            }
        ));
    }
}
