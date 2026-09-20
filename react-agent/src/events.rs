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
    ///
    /// `duration_ms` is wall-clock for the whole A2A call, so the UI can show a
    /// real per-agent timing instead of counting only the total round.
    ToolResult {
        agent: String,
        result: String,
        success: bool,
        turn: usize,
        duration_ms: u64,
    },

    /// A sub-agent's own progress update (e.g. its internal tool activity),
    /// relayed live while it works on a call from the orchestrator.
    ///
    /// This is free-form prose, because it is whatever the sub-agent chose to
    /// put in its WORKING status message. Agents that emit structured A2A data
    /// parts instead (`{type, tool_name, arguments, result}`) reach the UI
    /// directly through the stream's data-part channel and are rendered as
    /// real tool rows; this variant is the fallback for everything else.
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
