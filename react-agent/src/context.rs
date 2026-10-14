use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use std::collections::HashSet;

use nasiko_compress::Policy;
use serde::{Deserialize, Serialize};

/// What IP-3 removed from this turn's tool results, accumulated across every clone of one
/// [`ContextManager`].
///
/// **Shared, not a plain counter field.** The streaming path hands a *clone* of the manager to a
/// spawned task and drops it when the task ends (`react_loop.rs:696`, and the doc comment above
/// `run_stream` spelling out that `self.context` never sees a streaming turn's effects). A
/// per-instance field would therefore read zero for exactly the path that carries most of the
/// traffic — the failure would be a silent under-report, not a build error, which is why the
/// sharing is in the type rather than left to each call site.
///
/// Counts only, never content: the bytes are a size, and the tool output itself is already in the
/// agent's own trace.
#[derive(Debug, Clone, Default)]
pub struct CompressionTally(Arc<TallyCounts>);

#[derive(Debug, Default)]
struct TallyCounts {
    bytes_in: AtomicUsize,
    bytes_out: AtomicUsize,
    results: AtomicUsize,
}

impl CompressionTally {
    fn record(&self, before: usize, after: usize) {
        self.0.bytes_in.fetch_add(before, Ordering::Relaxed);
        self.0.bytes_out.fetch_add(after, Ordering::Relaxed);
        if after < before {
            self.0.results.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Tool-result bytes stored this turn, before compression.
    pub fn bytes_in(&self) -> usize {
        self.0.bytes_in.load(Ordering::Relaxed)
    }

    /// The same bytes as actually stored.
    pub fn bytes_out(&self) -> usize {
        self.0.bytes_out.load(Ordering::Relaxed)
    }

    /// Saturating. `nasiko-compress` guarantees it never grows its input, but the subtraction is
    /// written not to depend on that invariant holding somewhere else.
    pub fn saved_bytes(&self) -> usize {
        self.bytes_in().saturating_sub(self.bytes_out())
    }

    /// Tool results that actually came out smaller. "It ran and removed nothing" and "it never
    /// ran" are different facts, and only this distinguishes them once the byte counts match.
    pub fn results_compressed(&self) -> usize {
        self.0.results.load(Ordering::Relaxed)
    }
}

/// Configuration for context window management.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Max estimated tokens before triggering compaction.
    pub max_context_tokens: usize,
    /// Number of recent entries to always preserve verbatim.
    pub keep_recent: usize,
    /// Rough chars-per-token estimate for budget tracking.
    pub chars_per_token: usize,
    /// Structural compression applied to tool results as they are stored.
    ///
    /// This is the store, not the wire: it shrinks what the loop keeps, which also defers
    /// `max_context_tokens` compaction. The llm-router seam compresses what an agent *sends*;
    /// the two compose because compression is idempotent, so a payload that passes through
    /// both is compressed once.
    ///
    /// Disabled by default — an unconfigured loop stores exactly what it stored before.
    pub compress: Policy<'static>,
    /// Tools whose results are stored verbatim: agents with their Token optimization switch off.
    pub uncompressed_tools: HashSet<String>,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 80_000,
            keep_recent: 12,
            chars_per_token: 4,
            compress: Policy::default(),
            uncompressed_tools: HashSet::new(),
        }
    }
}

/// Manages conversation history for the ReAct loop.
/// Tracks entries, estimates token usage, and provides compaction.
#[derive(Clone)]
pub struct ContextManager {
    config: ContextConfig,
    entries: Vec<ContextEntry>,
    summary: Option<String>,
    estimated_tokens: usize,
    /// IP-3's measurement for this turn. Cloning the manager shares it; see [`CompressionTally`].
    tally: CompressionTally,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    pub role: ContextRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContextRole {
    User,
    Assistant,
    ToolResult { tool_name: String },
    System,
}

impl ContextManager {
    pub fn new(config: ContextConfig) -> Self {
        Self {
            config,
            entries: Vec::new(),
            summary: None,
            estimated_tokens: 0,
            tally: CompressionTally::default(),
        }
    }

    /// Handle onto this turn's IP-3 measurement, shared with every clone of this manager.
    ///
    /// Taken by the caller *before* the turn runs and read *after* it finishes: the streaming path
    /// compresses inside a spawned task, so there is no other moment at which both the numbers and
    /// the request's identity are in scope together.
    pub fn compression_tally(&self) -> CompressionTally {
        self.tally.clone()
    }

    pub fn push_user(&mut self, content: &str) {
        self.push(ContextEntry {
            role: ContextRole::User,
            content: content.to_string(),
        });
    }

    pub fn push_assistant(&mut self, content: &str) {
        self.push(ContextEntry {
            role: ContextRole::Assistant,
            content: content.to_string(),
        });
    }

    /// A batch containing any tool in `uncompressed_tools` is stored verbatim.
    pub fn push_tool_result(&mut self, tool_names: &[&str], content: &str) {
        let opted_out = tool_names
            .iter()
            .any(|name| self.config.uncompressed_tools.contains(*name));
        // Compress on the way in, so `push`'s token estimate — and therefore the compaction
        // trigger — sees the size the loop will actually carry.
        let stored = if opted_out {
            content.to_string()
        } else {
            nasiko_compress::compress(content, &self.config.compress).into_text()
        };
        self.tally.record(content.len(), stored.len());
        self.push(ContextEntry {
            role: ContextRole::ToolResult {
                tool_name: tool_names.join("+"),
            },
            content: stored,
        });
    }

    fn push(&mut self, entry: ContextEntry) {
        self.estimated_tokens += entry.content.len() / self.config.chars_per_token;
        self.entries.push(entry);
    }

    pub fn needs_compaction(&self) -> bool {
        self.estimated_tokens > self.config.max_context_tokens
    }

    /// Returns the current context window for the LLM.
    pub fn window(&self) -> ContextWindow {
        if self.entries.len() <= self.config.keep_recent {
            return ContextWindow {
                summary: self.summary.clone(),
                recent: self.entries.clone(),
            };
        }

        let start = self.entries.len() - self.config.keep_recent;
        ContextWindow {
            summary: self.summary.clone(),
            recent: self.entries[start..].to_vec(),
        }
    }

    /// Compact older entries into a summary string.
    /// In production, call `compaction_prompt()` and send to an LLM,
    /// then use `apply_summary()` with the result.
    pub fn compact_simple(&mut self) {
        if self.entries.len() <= self.config.keep_recent {
            return;
        }

        let split_at = self.entries.len() - self.config.keep_recent;
        let old: Vec<_> = self.entries.drain(..split_at).collect();

        let mut parts = Vec::new();
        if let Some(ref existing) = self.summary {
            parts.push(format!("[Prior summary]: {existing}"));
        }
        for entry in &old {
            let label = match &entry.role {
                ContextRole::User => "User".to_string(),
                ContextRole::Assistant => "Assistant".to_string(),
                ContextRole::ToolResult { tool_name } => format!("Tool({tool_name})"),
                ContextRole::System => "System".to_string(),
            };
            // Truncate by character count, not byte count — `entry.content` holds
            // LLM/tool output which is attacker-influenceable and may contain
            // multi-byte UTF-8 chars. A raw `&entry.content[..300]` byte slice can
            // land mid-character and panic ("byte index N is not a char boundary").
            let truncated = if entry.content.chars().count() > 300 {
                let head: String = entry.content.chars().take(300).collect();
                format!("{head}…")
            } else {
                entry.content.clone()
            };
            parts.push(format!("[{label}]: {truncated}"));
        }

        self.summary = Some(parts.join("\n"));
        self.recalculate_tokens();
    }

    /// Generates a prompt you can send to an LLM to produce a proper summary.
    pub fn compaction_prompt(&self) -> Option<String> {
        if self.entries.len() <= self.config.keep_recent {
            return None;
        }

        let split_at = self.entries.len() - self.config.keep_recent;
        let old = &self.entries[..split_at];

        let mut prompt = String::from(
            "Summarize this conversation history concisely. \
             Preserve: key facts, tool results, decisions made, and pending sub-tasks.\n\n",
        );

        if let Some(ref s) = self.summary {
            prompt.push_str(&format!("=== Prior Summary ===\n{s}\n\n"));
        }

        prompt.push_str("=== Messages ===\n");
        for entry in old {
            let label = match &entry.role {
                ContextRole::User => "User",
                ContextRole::Assistant => "Assistant",
                ContextRole::ToolResult { tool_name } => tool_name.as_str(),
                ContextRole::System => "System",
            };
            prompt.push_str(&format!("[{label}]: {}\n", entry.content));
        }

        Some(prompt)
    }

    /// Apply an LLM-generated summary and drop the old entries.
    pub fn apply_summary(&mut self, summary: String) {
        let split_at = self.entries.len().saturating_sub(self.config.keep_recent);
        if split_at > 0 {
            self.entries.drain(..split_at);
        }
        self.summary = Some(summary);
        self.recalculate_tokens();
    }

    fn recalculate_tokens(&mut self) {
        let entry_tokens: usize = self
            .entries
            .iter()
            .map(|e| e.content.len() / self.config.chars_per_token)
            .sum();
        let summary_tokens = self
            .summary
            .as_ref()
            .map(|s| s.len() / self.config.chars_per_token)
            .unwrap_or(0);
        self.estimated_tokens = entry_tokens + summary_tokens;
    }

    pub fn estimated_tokens(&self) -> usize {
        self.estimated_tokens
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

/// A snapshot of the context window to include in the LLM prompt.
#[derive(Debug, Clone)]
pub struct ContextWindow {
    pub summary: Option<String>,
    pub recent: Vec<ContextEntry>,
}

impl ContextWindow {
    /// Format as a string block for inclusion in a system prompt.
    pub fn format_for_prompt(&self) -> String {
        let mut out = String::new();

        if let Some(ref summary) = self.summary {
            out.push_str("<conversation-summary>\n");
            out.push_str(summary);
            out.push_str("\n</conversation-summary>\n\n");
        }

        if !self.recent.is_empty() {
            out.push_str("<recent-turns>\n");
            for entry in &self.recent {
                let tag = match &entry.role {
                    ContextRole::User => "user",
                    ContextRole::Assistant => "assistant",
                    ContextRole::ToolResult { tool_name } => tool_name.as_str(),
                    ContextRole::System => "system",
                };
                out.push_str(&format!("<{tag}>{}</{tag}>\n", entry.content));
            }
            out.push_str("</recent-turns>");
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_keep_recent_config() -> ContextConfig {
        ContextConfig {
            max_context_tokens: 1_000_000,
            keep_recent: 1,
            chars_per_token: 4,
            compress: Policy::default(),
            uncompressed_tools: HashSet::new(),
        }
    }

    // ── IP-3: tool-result compression ───────────────────────────────────────

    /// 300 lines of timestamped log — the shape a tool result actually takes.
    fn noisy_tool_result() -> String {
        (0..300)
            .map(|i| format!("2026-01-01T00:00:00Z INFO handled request {i}\n"))
            .collect()
    }

    fn compressing_config() -> ContextConfig {
        ContextConfig {
            compress: Policy {
                enabled: true,
                min_bytes: 0,
                ..Policy::default()
            },
            ..ContextConfig::default()
        }
    }

    #[test]
    fn tool_results_are_stored_verbatim_by_default() {
        // The zero-behaviour-change guard: an unconfigured loop must store what it always did.
        let payload = noisy_tool_result();
        let mut ctx = ContextManager::new(ContextConfig::default());

        ctx.push_tool_result(&["fetch_logs"], &payload);

        assert_eq!(ctx.entries.last().unwrap().content, payload);
    }

    #[test]
    fn enabling_compression_shrinks_what_is_stored() {
        let payload = noisy_tool_result();
        let mut ctx = ContextManager::new(compressing_config());

        ctx.push_tool_result(&["fetch_logs"], &payload);

        let stored = &ctx.entries.last().unwrap().content;
        assert!(
            stored.len() < payload.len(),
            "stored {} bytes, original {}",
            stored.len(),
            payload.len()
        );
    }

    fn opting_out(tool: &str) -> ContextConfig {
        ContextConfig {
            uncompressed_tools: HashSet::from([tool.to_string()]),
            ..compressing_config()
        }
    }

    #[test]
    fn an_opted_out_agent_does_not_stop_compression_for_the_others() {
        let payload = noisy_tool_result();
        let mut ctx = ContextManager::new(opting_out("call_agent_a"));

        ctx.push_tool_result(&["call_agent_a"], &payload);
        assert_eq!(ctx.entries.last().unwrap().content, payload);

        ctx.push_tool_result(&["call_agent_b"], &payload);
        assert!(ctx.entries.last().unwrap().content.len() < payload.len());
    }

    #[test]
    fn a_batch_containing_an_opted_out_agent_is_stored_verbatim() {
        let payload = noisy_tool_result();
        let mut ctx = ContextManager::new(opting_out("call_agent_a"));

        ctx.push_tool_result(&["call_agent_b", "call_agent_a"], &payload);

        let entry = ctx.entries.last().unwrap();
        assert_eq!(entry.content, payload);
        assert!(
            matches!(&entry.role, ContextRole::ToolResult { tool_name } if tool_name == "call_agent_b+call_agent_a")
        );
    }

    // ── IP-3: the savings tally ─────────────────────────────────────────────

    /// The property the whole shared-handle design exists for. `run_stream` compresses inside a
    /// spawned task holding a **clone** of the manager and drops it; if the tally were a plain
    /// field, the handle the caller kept would read zero and IP-3 would silently report nothing on
    /// the path that carries most of the traffic.
    #[test]
    fn a_clone_compressing_is_visible_to_the_handle_taken_before_it() {
        let payload = noisy_tool_result();
        let ctx = ContextManager::new(compressing_config());

        // Taken before the "turn", exactly as the dispatcher takes it.
        let tally = ctx.compression_tally();

        let mut spawned = ctx.clone();
        spawned.push_tool_result(&["fetch_logs"], &payload);
        drop(spawned);

        assert!(
            tally.saved_bytes() > 0,
            "the clone's compression was invisible to the handle: in {} out {}",
            tally.bytes_in(),
            tally.bytes_out()
        );
        assert_eq!(tally.bytes_in(), payload.len());
        assert_eq!(tally.results_compressed(), 1);
    }

    #[test]
    fn the_tally_sums_every_tool_result_in_a_turn() {
        let payload = noisy_tool_result();
        let mut ctx = ContextManager::new(compressing_config());
        let tally = ctx.compression_tally();

        ctx.push_tool_result(&["fetch_logs"], &payload);
        ctx.push_tool_result(&["fetch_logs"], &payload);

        assert_eq!(tally.bytes_in(), payload.len() * 2);
        assert_eq!(tally.results_compressed(), 2);
    }

    /// With IP-3 off the emitter must stay silent rather than write a row of zeros — `saved_bytes`
    /// is what it gates on, and `results_compressed` is what tells "off" apart from "ran, found
    /// nothing".
    #[test]
    fn compression_off_tallies_bytes_but_no_saving() {
        let payload = noisy_tool_result();
        let mut ctx = ContextManager::new(ContextConfig::default());
        let tally = ctx.compression_tally();

        ctx.push_tool_result(&["fetch_logs"], &payload);

        assert_eq!(tally.bytes_in(), payload.len());
        assert_eq!(tally.bytes_in(), tally.bytes_out());
        assert_eq!(tally.saved_bytes(), 0);
        assert_eq!(tally.results_compressed(), 0);
    }

    /// A fresh conversation starts a fresh measurement — `reset_context` builds a new manager, so
    /// a handle taken from the old one must not keep collecting.
    #[test]
    fn a_new_manager_starts_a_new_tally() {
        let payload = noisy_tool_result();
        let mut first = ContextManager::new(compressing_config());
        let old_tally = first.compression_tally();
        first.push_tool_result(&["fetch_logs"], &payload);

        let mut second = ContextManager::new(compressing_config());
        second.push_tool_result(&["fetch_logs"], &payload);

        assert_eq!(
            old_tally.results_compressed(),
            1,
            "the old turn's count moved"
        );
    }

    #[test]
    fn compression_defers_the_compaction_cliff() {
        // The quality claim in PRD §9 IP-3: a smaller store reaches `max_context_tokens` later.
        let payload = noisy_tool_result();

        let mut plain = ContextManager::new(ContextConfig::default());
        let mut compressed = ContextManager::new(compressing_config());
        for _ in 0..20 {
            plain.push_tool_result(&["fetch_logs"], &payload);
            compressed.push_tool_result(&["fetch_logs"], &payload);
        }

        assert!(
            compressed.estimated_tokens < plain.estimated_tokens,
            "compressed {} vs plain {} estimated tokens",
            compressed.estimated_tokens,
            plain.estimated_tokens
        );
    }

    #[test]
    fn a_second_pass_at_the_router_seam_would_be_a_no_op() {
        // IP-3 and IP-1 both run on the same bytes when an agent forwards a stored result.
        // Idempotence (invariant I3) is what keeps that from compounding into a second elision.
        let payload = noisy_tool_result();
        let mut ctx = ContextManager::new(compressing_config());
        ctx.push_tool_result(&["fetch_logs"], &payload);
        let stored = ctx.entries.last().unwrap().content.clone();

        let policy = Policy {
            enabled: true,
            min_bytes: 0,
            ..Policy::default()
        };
        let again = nasiko_compress::compress(&stored, &policy);

        assert_eq!(again.into_text(), stored);
    }

    #[test]
    fn dry_run_measures_without_changing_the_store() {
        let payload = noisy_tool_result();
        let mut ctx = ContextManager::new(ContextConfig {
            compress: Policy {
                enabled: true,
                min_bytes: 0,
                dry_run: true,
                ..Policy::default()
            },
            ..ContextConfig::default()
        });

        ctx.push_tool_result(&["fetch_logs"], &payload);

        assert_eq!(ctx.entries.last().unwrap().content, payload);
    }

    #[test]
    fn compact_simple_truncates_multibyte_content_without_panicking() {
        let mut ctx = ContextManager::new(small_keep_recent_config());

        // 299 ASCII chars followed by a 3-byte '€' character then more ASCII.
        // Byte index 300 lands mid-way through the '€' character's UTF-8
        // encoding — a naive `&content[..300]` byte slice panics here with
        // "byte index 300 is not a char boundary". Truncating by character
        // count instead must not panic.
        let content = format!("{}{}", "a".repeat(299), "€bbbb");
        assert!(content.chars().count() > 300);

        ctx.push_tool_result(&["tool"], &content);
        // A second, recent entry so compact_simple() has something to fold
        // into the summary (the tool result) and something to keep verbatim.
        ctx.push_user("recent turn");

        ctx.compact_simple(); // must not panic

        let summary = ctx.window().summary.expect("summary should be set");
        assert!(
            summary.contains('…'),
            "expected truncation marker in summary: {summary}"
        );
        assert!(
            !summary.contains("bbbb"),
            "content past the 300-char cutoff should have been dropped: {summary}"
        );
    }

    #[test]
    fn compact_simple_leaves_short_content_untruncated() {
        let mut ctx = ContextManager::new(small_keep_recent_config());
        ctx.push_tool_result(&["tool"], "short result");
        ctx.push_user("recent turn");

        ctx.compact_simple();

        let summary = ctx.window().summary.expect("summary should be set");
        assert!(summary.contains("short result"));
        assert!(!summary.contains('…'));
    }
}
