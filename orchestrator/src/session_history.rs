use std::collections::HashSet;

use nasiko_compress::Policy;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::context_selection::ContextSelectionStrategy;
use crate::pacms_selector::PacmsSelector;
use crate::vector_store::{VectorStore, cosine_similarity};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// One user query paired with the assistant's reply that followed it, in a
/// session's chronological message stream. This is the candidate unit for
/// the `topk` context-selection strategy (`SessionHistory::fetch_topk`).
#[derive(Debug, Clone)]
pub struct MessagePair {
    pub query: String,
    pub answer: String,
}

/// The `chat_messages.metadata` key marking a row as an orchestrator refusal,
/// which keeps it out of the next turn's reasoning context.
///
/// Exported because the **writer lives in another crate** (`nasiko_server`'s
/// `usage_meta::insert_assistant_message`) while the only reader is the SQL in
/// [`SessionHistory::fetch`]. The two were independent string literals, so a
/// typo or a rename on either side would have silently stopped the filter
/// matching — and the symptom is not an error, it is the live bug this tag
/// exists to prevent quietly coming back: refusals re-enter the prompt and the
/// session teaches itself to keep refusing. `the_filter_reads_the_key_the_writer_writes`
/// pins the SQL against this constant.
pub const REFUSAL_METADATA_KEY: &str = "orchestrator_refusal";

/// The history query. A `const` rather than an inline literal so the test below
/// can assert it actually mentions [`REFUSAL_METADATA_KEY`].
const FETCH_HISTORY_SQL: &str = "SELECT role, content FROM chat_messages \
     WHERE session_id = $1 \
       AND NOT COALESCE((metadata->>'orchestrator_refusal')::boolean, false) \
     ORDER BY timestamp DESC LIMIT $2";

#[derive(Debug, Clone, Default)]
pub struct SessionHistory {
    pub messages: Vec<ChatMessage>,
}

/// Per-call tuning knobs for [`SessionHistory::fetch_context`], bundled so
/// the dispatcher's argument count stays reasonable. Built for callers by
/// `ContextTiers::resolve` — nothing outside this crate constructs one, so
/// the operator-configured tier table is the only way in.
pub(crate) struct ContextFetchConfig {
    /// Candidate window both embedding-backed strategies draw from: the pool
    /// `fetch_pacms` selects a budget-fitting subset of, and the pool
    /// `fetch_topk` pairs up and ranks. Ignored by `LastK`, which is sized by
    /// `lastk_limit` alone.
    pub pool_size: usize,
    /// `fetch_pacms`'s token budget, already resolved from the user's
    /// `PacmsBudgetLevel` tier (ignored by `TopK`/`LastK`).
    pub token_budget: usize,
    /// `fetch_pacms`'s mandatory-recent window (ignored by `TopK`/`LastK`).
    pub mandatory_recent: usize,
    /// Number of most-relevant pairs `fetch_topk` keeps — resolved from the
    /// same `PacmsBudgetLevel` tier as `token_budget` above, via
    /// `PacmsBudgetLevel::k` (ignored by `Pacms`).
    pub topk_count: usize,
    /// Recency window for the standalone `LastK` strategy, and for `TopK`'s
    /// fallback when embeddings are unavailable or the session has no
    /// pairs — same tier-derived value as `topk_count` (ignored by `Pacms`).
    pub lastk_limit: usize,
    /// Structural compression applied to each message as it is read, before
    /// any strategy scores, embeds or truncates it (IP-4). Disabled by
    /// default, so an unconfigured deployment selects over exactly the text it
    /// selected over before.
    pub compress: Policy<'static>,
}

impl SessionHistory {
    /// Take the LATEST `limit` messages, then restore chronological order —
    /// `ORDER BY timestamp ASC LIMIT n` would pin the window to the oldest
    /// messages and never advance in long sessions.
    /// `compress` runs here, at the read, and nowhere later. That ordering is the whole of
    /// IP-4's correctness: every strategy downstream — PACMS's token budget, TopK's embeddings,
    /// LastK's window — measures the text it is handed. Compress after selection and the
    /// selector fills a tier's budget with full-size text that then halves, so a user silently
    /// receives half the context tier they chose. Compress before it, and the same budget holds
    /// more turns, which is the saving.
    async fn fetch_raw(
        session_id: &str,
        pool: &PgPool,
        limit: usize,
        compress: &Policy<'_>,
    ) -> Vec<ChatMessage> {
        // Rows tagged `orchestrator_refusal` are excluded from reasoning context
        // on purpose. A refusal is persisted so the human still sees it in the
        // transcript, but feeding it back as prior assistant output teaches the
        // model that refusing is what this conversation does — observed live as a
        // session that refused once and then refused every following turn,
        // including ones a deployed agent plainly covered. The row stays in
        // `chat_messages`; it just never becomes part of the next turn's prompt.
        // Nothing here writes that tag: it is set by whatever policy produced the
        // refusal, and with no policy configured no row ever carries it.
        let (mut bytes_in, mut bytes_out, mut touched) = (0usize, 0usize, 0usize);
        let mut messages: Vec<ChatMessage> =
            sqlx::query_as::<_, (String, String)>(FETCH_HISTORY_SQL)
                .bind(session_id)
                .bind(limit as i64)
                .fetch_all(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(role, content)| {
                    let before = content.len();
                    let content = nasiko_compress::compress(&content, compress).into_text();
                    bytes_in += before;
                    bytes_out += content.len();
                    if content.len() < before {
                        touched += 1;
                    }
                    ChatMessage { role, content }
                })
                .collect();

        // IP-4 had no per-request record of any kind, so a run could not be told apart from one
        // where it did nothing — the difference only showed up as a token delta two layers away.
        // Emitted at INFO and keyed by session so a single run can be audited from the logs.
        if touched > 0 {
            tracing::info!(
                target: "nasiko::orchestrator::history_compress",
                session_id,
                bytes_in,
                bytes_out,
                messages_touched = touched,
                saved_pct = (100.0 * (bytes_in - bytes_out) as f64 / bytes_in.max(1) as f64),
                "history compressed before selection"
            );
        }
        messages.reverse();
        messages
    }

    /// Plain recency read, uncompressed.
    ///
    /// This is the fixed-window path (HITL resume, and anything else reading history directly
    /// rather than through a resolved tier), not a budgeted selection. It is deliberately left
    /// verbatim: the resume prompt is the one turn where the model is told *not* to call anyone
    /// again, so eliding the history it is reasoning over has no second chance to recover.
    /// Compression reaches history through [`Self::fetch_context`], which carries a policy.
    pub async fn fetch(session_id: &str, pool: &PgPool, limit: usize) -> Self {
        Self {
            messages: Self::fetch_raw(session_id, pool, limit, &Policy::default()).await,
        }
    }

    /// PACMS-selected context: pulls a wider `pool_size` window of recent
    /// messages, then uses `PacmsSelector` to pick a `token_budget`-fitting,
    /// query-relevant, coverage-diversified subset instead of plain
    /// recency truncation — the most recent `mandatory_recent` messages in
    /// the pool are always kept, so the immediate thread is never dropped.
    ///
    /// Falls back to `select_lastk` (no embeddings) if selection fails (e.g.
    /// the embeddings API is down) so a transient failure degrades to the
    /// old recency behavior instead of breaking the request.
    // Eight, because the knobs are passed individually: `ContextFetchConfig` bundles exactly
    // these for the dispatcher, but it is `pub(crate)` and this entry point is public.
    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_pacms(
        session_id: &str,
        pool: &PgPool,
        vector_store: &VectorStore,
        query: &str,
        pool_size: usize,
        token_budget: usize,
        mandatory_recent: usize,
        compress: &Policy<'_>,
    ) -> Self {
        let messages = Self::fetch_raw(session_id, pool, pool_size, compress).await;
        if messages.is_empty() {
            return Self { messages };
        }

        let candidates: Vec<String> = messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect();

        let n = candidates.len();
        let mandatory: HashSet<usize> = (n.saturating_sub(mandatory_recent)..n).collect();

        let selector = PacmsSelector::new(vector_store);
        let selected = match selector
            .select_pacms(&candidates, query, token_budget, Some(&mandatory))
            .await
        {
            Ok(idx) => idx,
            Err(e) => {
                tracing::warn!(%e, "PACMS context selection failed — falling back to recency selection");
                selector.select_lastk(&candidates, token_budget, Some(&mandatory))
            }
        };

        let mut selected = selected;
        selected.sort_unstable();
        let selected_set: HashSet<usize> = selected.iter().copied().collect();

        tracing::debug!(
            target: "pacms_context",
            session_id,
            query,
            selected_count = selected.len(),
            pool_size = n,
            "PACMS context selection for query — kept {}/{} pooled messages",
            selected.len(),
            n
        );
        for (i, m) in messages.iter().enumerate() {
            let kept = selected_set.contains(&i);
            tracing::debug!(
                target: "pacms_context",
                pool_index = i,
                kept,
                mandatory = mandatory.contains(&i),
                role = %m.role,
                content = %m.content,
                "{} [{i}] {}: {}",
                if kept { "KEPT" } else { "DROP" },
                m.role,
                m.content
            );
        }

        let messages: Vec<ChatMessage> = selected
            .into_iter()
            .filter_map(|i| messages.get(i).cloned())
            .collect();

        Self { messages }
    }

    /// Dispatch to the user's selected context-selection strategy.
    ///
    /// `TopK` falls back to plain recency (`fetch`, sized by
    /// `cfg.lastk_limit`) if embeddings are unavailable or the session has
    /// no complete pairs yet — the same "a transient failure degrades to
    /// recency instead of breaking the request" contract `fetch_pacms`
    /// already has via its own internal `select_lastk` fallback.
    pub(crate) async fn fetch_context(
        strategy: ContextSelectionStrategy,
        session_id: &str,
        pool: &PgPool,
        vector_store: &VectorStore,
        query: &str,
        cfg: &ContextFetchConfig,
    ) -> Self {
        match strategy {
            ContextSelectionStrategy::Pacms => {
                Self::fetch_pacms(
                    session_id,
                    pool,
                    vector_store,
                    query,
                    cfg.pool_size,
                    cfg.token_budget,
                    cfg.mandatory_recent,
                    &cfg.compress,
                )
                .await
            }
            ContextSelectionStrategy::TopK => {
                let history = Self::fetch_topk(
                    session_id,
                    pool,
                    query,
                    vector_store,
                    cfg.topk_count,
                    cfg.pool_size,
                    &cfg.compress,
                )
                .await;
                if history.is_empty() {
                    Self::fetch(session_id, pool, cfg.lastk_limit).await
                } else {
                    history
                }
            }
            ContextSelectionStrategy::LastK => Self::fetch(session_id, pool, cfg.lastk_limit).await,
        }
    }

    /// Fetch the `top_k` message pairs most relevant to `query`, ranked by
    /// cosine similarity of their (query + answer) embedding to the query's
    /// embedding — most relevant first. Unlike `fetch`/`fetch_pacms`, the
    /// result is *not* restored to chronological order, and there is no
    /// token budget or mandatory-recent floor: this is a faithful port of
    /// the plain top-k-by-relevance baseline.
    ///
    /// Returns an empty history if the session has no complete pairs, or if
    /// embedding fails for any reason (disabled vector store, API error,
    /// mismatched response) — `fetch_context` falls back to `fetch` for the
    /// `TopK` strategy when this happens.
    pub async fn fetch_topk(
        session_id: &str,
        pool: &PgPool,
        query: &str,
        vector_store: &VectorStore,
        top_k: usize,
        pool_size: usize,
        compress: &Policy<'_>,
    ) -> Self {
        let pairs = Self::fetch_pairs(session_id, pool, pool_size, compress).await;
        if pairs.is_empty() {
            return Self::default();
        }

        // Embed query+answer concatenated per pair, so a pair scores as
        // relevant if either half matches the current query.
        let pair_texts: Vec<String> = pairs
            .iter()
            .map(|p| format!("{} {}", p.query, p.answer))
            .collect();

        // One call for the query, one batched call for every pair.
        let query_embedding = vector_store.embed(query).await;
        let pair_embeddings = vector_store.embed_batch(&pair_texts).await;

        let (query_embedding, pair_embeddings) = match (query_embedding, pair_embeddings) {
            (Ok(q), Ok(p)) => (q, p),
            (Err(e), _) | (_, Err(e)) => {
                tracing::warn!(%e, "top-k context selection failed — embeddings unavailable");
                return Self::default();
            }
        };

        Self {
            messages: rank_pairs(&pairs, &pair_embeddings, &query_embedding, top_k),
        }
    }

    /// Pair up each `user` message with the `assistant` message that
    /// immediately follows it, over the latest `pool_size` messages.
    /// Unmatched trailing/leading messages (e.g. a query the assistant hasn't
    /// answered yet, or non user/assistant roles) are skipped.
    ///
    /// Bounded by the same `pool_size` window `fetch_pacms` draws from: this
    /// used to select the session's entire history with no `LIMIT` and embed
    /// every pair of it, so a long-running session grew both the query and
    /// the per-request embedding cost without limit.
    /// Compresses at the read for the same reason `fetch_raw` does: the pair text below is what
    /// gets embedded and ranked, so it has to be the text that will actually be sent.
    async fn fetch_pairs(
        session_id: &str,
        pool: &PgPool,
        pool_size: usize,
        compress: &Policy<'_>,
    ) -> Vec<MessagePair> {
        // Latest `pool_size` (DESC + LIMIT), then reversed back into
        // chronological order so the pairing below sees user→assistant
        // adjacency — `ORDER BY timestamp ASC LIMIT n` would pin the window to
        // the oldest messages and never advance, the same trap `fetch_raw`
        // documents.
        let mut messages: Vec<(String, String)> = sqlx::query_as::<_, (String, String)>(
            "SELECT role, content FROM chat_messages \
             WHERE session_id = $1 ORDER BY timestamp DESC LIMIT $2",
        )
        .bind(session_id)
        .bind(pool_size as i64)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        messages.reverse();

        let mut pairs = Vec::new();
        let mut i = 0;
        while i + 1 < messages.len() {
            let (role_a, content_a) = &messages[i];
            let (role_b, content_b) = &messages[i + 1];
            if role_a == "user" && role_b == "assistant" {
                pairs.push(MessagePair {
                    query: nasiko_compress::compress(content_a, compress).into_text(),
                    answer: nasiko_compress::compress(content_b, compress).into_text(),
                });
                i += 2; // consume both messages of the pair
            } else {
                i += 1; // not a user→assistant pair here — slide the window by one
            }
        }
        pairs
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Prior user turns in the fetched window — a coarse, capped count (see
    /// `fetch`'s `limit`), used as a session-shape signal for the
    /// minimal-code ladder rather than an exact lifetime turn count.
    pub fn user_turn_count(&self) -> usize {
        self.messages.iter().filter(|m| m.role == "user").count()
    }

    /// Map to LLM-format messages (role + content pairs).
    pub fn to_llm_messages(&self) -> Vec<LlmMessage> {
        self.messages
            .iter()
            .map(|m| LlmMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect()
    }

    /// Flat text of all messages — used as Stage 2 embedding input for re-ranking.
    pub fn summary_text(&self) -> String {
        self.messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Build the full query string: history context + current message.
    pub fn with_current_query(&self, query: &str) -> String {
        // Drop a trailing user turn that *is* `query`. `agent_proxy` persists the incoming
        // message from a `tokio::spawn`, so whether it has landed by the time history is read is
        // a race: when the insert wins, the message comes back as history and is then appended
        // again here, sending the whole payload twice. On an 80 KB log that doubled the turn's
        // input tokens. Deduplicating on content makes the result identical either way, rather
        // than depending on which task won.
        let deduped: Vec<&ChatMessage> = {
            let mut msgs: Vec<&ChatMessage> = self.messages.iter().collect();
            if msgs
                .last()
                .is_some_and(|m| m.role == "user" && m.content == query)
            {
                msgs.pop();
            }
            msgs
        };

        if deduped.is_empty() {
            query.to_string()
        } else {
            let history = deduped
                .iter()
                .map(|m| format!("{}: {}", m.role, m.content))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{history}\n\nCurrent message: {query}")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

#[cfg(test)]
mod refusal_tag_tests {
    use super::*;

    /// The writer of this tag is in another crate. Nothing but this assertion
    /// ties the two spellings together, and a mismatch fails silently — the
    /// filter simply stops matching and refusals flow back into the prompt.
    #[test]
    fn the_filter_reads_the_key_the_writer_writes() {
        assert!(
            FETCH_HISTORY_SQL.contains(REFUSAL_METADATA_KEY),
            "the history query must filter on `{REFUSAL_METADATA_KEY}`: {FETCH_HISTORY_SQL}"
        );
    }
}

/// The pure selection core of `fetch_topk`: no I/O, so it can be exercised
/// with fabricated embeddings instead of a live embeddings endpoint. Scores
/// every pair by cosine similarity to `query_embedding`, sorts descending,
/// and flattens the top `top_k` pairs into `[user, assistant]` messages —
/// in similarity-rank order, not chronological order.
fn rank_pairs(
    pairs: &[MessagePair],
    pair_embeddings: &[Vec<f32>],
    query_embedding: &[f32],
    top_k: usize,
) -> Vec<ChatMessage> {
    let mut scored: Vec<(f32, &MessagePair)> = pairs
        .iter()
        .zip(pair_embeddings.iter())
        .map(|(pair, emb)| (cosine_similarity(query_embedding, emb), pair))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    scored
        .into_iter()
        .take(top_k)
        .flat_map(|(_, pair)| {
            [
                ChatMessage {
                    role: "user".to_string(),
                    content: pair.query.clone(),
                },
                ChatMessage {
                    role: "assistant".to_string(),
                    content: pair.answer.clone(),
                },
            ]
        })
        .collect()
}

#[cfg(test)]
mod rank_pairs_tests {
    use super::*;

    fn pair(query: &str, answer: &str) -> MessagePair {
        MessagePair {
            query: query.to_string(),
            answer: answer.to_string(),
        }
    }

    #[test]
    fn ranks_by_similarity_not_recency() {
        // Oldest pair first in the input, but its embedding is closest to
        // the query — it must come out first, not last.
        let pairs = vec![
            pair("refund status", "processed yesterday"),
            pair("shipping estimate", "3-5 business days"),
            pair("login help", "reset your password"),
        ];
        let embeddings = vec![
            vec![1.0, 0.0], // "refund" — closest to the query below
            vec![0.0, 1.0], // "shipping" — orthogonal
            vec![0.5, 0.5], // "login" — partial overlap
        ];
        let query_embedding = vec![1.0, 0.0];

        let messages = rank_pairs(&pairs, &embeddings, &query_embedding, 2);

        assert_eq!(messages.len(), 4); // top_k=2 pairs * 2 messages each
        assert_eq!(messages[0].content, "refund status");
        assert_eq!(messages[1].content, "processed yesterday");
        // Second-ranked by cosine similarity is "login" (0.5,0.5), not the
        // chronologically-second "shipping" (0.0,1.0).
        assert_eq!(messages[2].content, "login help");
        assert_eq!(messages[3].content, "reset your password");
    }

    #[test]
    fn top_k_larger_than_pool_returns_everything() {
        let pairs = vec![pair("a", "b")];
        let embeddings = vec![vec![1.0, 0.0]];
        let messages = rank_pairs(&pairs, &embeddings, &[1.0, 0.0], 10);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn empty_pool_returns_empty() {
        let messages = rank_pairs(&[], &[], &[1.0, 0.0], 5);
        assert!(messages.is_empty());
    }
}

#[cfg(test)]
mod ip4_tests {
    use super::*;
    use crate::pacms_selector::PacmsSelector;
    use crate::vector_store::VectorStore;

    /// 300 lines of timestamped log, the shape a pasted terminal dump takes in a chat turn.
    fn noisy_turn() -> String {
        (0..300)
            .map(|i| format!("2026-01-01T00:00:00Z INFO handled request {i}\n"))
            .collect()
    }

    fn on() -> Policy<'static> {
        Policy {
            enabled: true,
            min_bytes: 0,
            ..Policy::default()
        }
    }

    #[test]
    fn disabled_by_default_so_selection_sees_exactly_what_it_saw_before() {
        let turn = noisy_turn();
        let out = nasiko_compress::compress(&turn, &Policy::default()).into_text();
        assert_eq!(out, turn);
    }

    /// The reason IP-4 compresses at the read rather than after selection (PRD §9).
    ///
    /// Compressing first means the tier's budget is spent on compressed turns, so *more* of them
    /// fit. Compressing afterwards would fill the budget with full-size text and then halve it,
    /// silently giving the user less context than the tier they chose.
    #[test]
    fn compressing_before_selection_fits_more_turns_in_the_same_budget() {
        let vs = VectorStore::disabled();
        let selector = PacmsSelector::new(&vs);
        const BUDGET: usize = 2_000;

        let raw: Vec<String> = (0..8).map(|_| noisy_turn()).collect();
        let compressed: Vec<String> = raw
            .iter()
            .map(|t| nasiko_compress::compress(t, &on()).into_text())
            .collect();

        let kept_raw = selector.select_lastk(&raw, BUDGET, None).len();
        let kept_compressed = selector.select_lastk(&compressed, BUDGET, None).len();

        assert!(
            kept_compressed > kept_raw,
            "same {BUDGET}-token budget kept {kept_compressed} compressed vs {kept_raw} raw turns"
        );
    }

    /// History flows on into the router seam, which compresses again. Without idempotence the
    /// second pass would elide an already-elided transcript.
    #[test]
    fn history_survives_a_second_pass_at_the_router_seam() {
        let once = nasiko_compress::compress(&noisy_turn(), &on()).into_text();
        let twice = nasiko_compress::compress(&once, &on()).into_text();
        assert_eq!(once, twice);
    }

    /// `fetch` is the HITL resume read. Its prompt tells the model not to delegate again, so an
    /// elision there has no second chance to be recovered — it stays verbatim by construction.
    #[test]
    fn the_fixed_window_read_carries_no_policy() {
        let src = include_str!("session_history.rs");
        let marker = "messages: Self::fetch_raw(session_id, pool, limit, &Policy::default()).await";
        assert!(
            src.contains(marker),
            "`fetch` must keep reading history uncompressed"
        );
    }
}

#[cfg(test)]
mod current_query_dedupe_tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
        }
    }

    fn history(messages: Vec<ChatMessage>) -> SessionHistory {
        SessionHistory { messages }
    }

    #[test]
    fn a_trailing_copy_of_the_current_query_is_not_sent_twice() {
        // The race `agent_proxy`'s spawned insert creates: the incoming message has already
        // landed in `chat_messages` by the time history is read.
        let h = history(vec![
            msg("user", "first question"),
            msg("assistant", "first answer"),
            msg("user", "ANALYSE THIS HUGE LOG"),
        ]);

        let out = h.with_current_query("ANALYSE THIS HUGE LOG");

        assert_eq!(
            out.matches("ANALYSE THIS HUGE LOG").count(),
            1,
            "sent twice: {out}"
        );
        assert!(out.contains("first question"), "real history must survive");
        assert!(out.ends_with("Current message: ANALYSE THIS HUGE LOG"));
    }

    #[test]
    fn the_same_turn_costs_the_same_whether_or_not_the_insert_landed() {
        // The point of the fix: the outgoing text must not depend on who won the race.
        let base = vec![msg("user", "q1"), msg("assistant", "a1")];
        let mut raced = base.clone();
        raced.push(msg("user", "current"));

        assert_eq!(
            history(base).with_current_query("current"),
            history(raced).with_current_query("current")
        );
    }

    #[test]
    fn an_earlier_identical_question_is_still_kept() {
        // Only the *trailing* copy is the race artifact. A genuine repeat earlier in the
        // conversation is real history and must not be silently dropped.
        let h = history(vec![msg("user", "same"), msg("assistant", "answer")]);

        let out = h.with_current_query("same");

        assert_eq!(
            out.matches("same").count(),
            2,
            "earlier turn was eaten: {out}"
        );
    }

    #[test]
    fn a_trailing_assistant_message_is_never_dropped() {
        let h = history(vec![msg("user", "q"), msg("assistant", "echo")]);

        let out = h.with_current_query("echo");

        assert!(
            out.contains("assistant: echo"),
            "assistant turn dropped: {out}"
        );
    }

    #[test]
    fn empty_history_still_returns_the_bare_query() {
        assert_eq!(history(vec![]).with_current_query("hello"), "hello");
    }

    #[test]
    fn history_of_only_the_duplicate_returns_the_bare_query() {
        let h = history(vec![msg("user", "hello")]);
        assert_eq!(h.with_current_query("hello"), "hello");
    }
}
