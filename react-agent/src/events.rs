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

    /// A sub-agent's own progress update (e.g. its internal tool activity),
    /// relayed live while it works on a call from the orchestrator.
    SubStatus { agent: String, message: String },

    /// A chunk of a sub-agent's reply text as it generates, relayed live.
    /// The full reply still arrives as `ToolResult` when the call finishes.
    SubContent { agent: String, content: String },

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

    /// Token usage from an LLM call. Non-streaming turns report exact
    /// provider counts; streamed turns report a character-based estimate
    /// (`estimated: true`) because the rig 0.11 stream surfaces no usage
    /// chunk — consumers must label estimated figures as approximate.
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        model: String,
        estimated: bool,
    },

    /// An error occurred during orchestration.
    Error { message: String },
}
