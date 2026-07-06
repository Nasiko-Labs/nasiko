use serde::{Deserialize, Serialize};

/// Configuration for context window management.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Max estimated tokens before triggering compaction.
    pub max_context_tokens: usize,
    /// Number of recent entries to always preserve verbatim.
    pub keep_recent: usize,
    /// Rough chars-per-token estimate for budget tracking.
    pub chars_per_token: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 80_000,
            keep_recent: 12,
            chars_per_token: 4,
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
        }
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

    pub fn push_tool_result(&mut self, tool_name: &str, content: &str) {
        self.push(ContextEntry {
            role: ContextRole::ToolResult {
                tool_name: tool_name.to_string(),
            },
            content: content.to_string(),
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
        }
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

        ctx.push_tool_result("tool", &content);
        // A second, recent entry so compact_simple() has something to fold
        // into the summary (the tool result) and something to keep verbatim.
        ctx.push_user("recent turn");

        ctx.compact_simple(); // must not panic

        let summary = ctx.window().summary.expect("summary should be set");
        assert!(summary.contains('…'), "expected truncation marker in summary: {summary}");
        assert!(
            !summary.contains("bbbb"),
            "content past the 300-char cutoff should have been dropped: {summary}"
        );
    }

    #[test]
    fn compact_simple_leaves_short_content_untruncated() {
        let mut ctx = ContextManager::new(small_keep_recent_config());
        ctx.push_tool_result("tool", "short result");
        ctx.push_user("recent turn");

        ctx.compact_simple();

        let summary = ctx.window().summary.expect("summary should be set");
        assert!(summary.contains("short result"));
        assert!(!summary.contains('…'));
    }
}
