use serde::Serialize;

/// Events emitted during orchestration, streamed to the caller in real-time.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestratorEvent {
    /// Orchestrator is reasoning about what to do next.
    Thinking { content: String },

    /// About to call an agent tool.
    ToolCall {
        agent: String,
        message: String,
        turn: usize,
    },

    /// Agent returned a result.
    ToolResult {
        agent: String,
        result: String,
        success: bool,
        turn: usize,
    },

    /// A chunk of the final response text.
    Content { content: String },

    /// Orchestration completed.
    Done {
        turns: usize,
        context_compacted: bool,
    },

    /// A call was blocked by flow policy (cycle, depth, budget, timeout).
    PolicyRejected {
        agent: String,
        reason: String,
        turn: usize,
    },

    /// Token usage from an LLM call (emitted per non-streaming turn).
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        model: String,
    },

    /// An error occurred during orchestration.
    Error { message: String },
}
