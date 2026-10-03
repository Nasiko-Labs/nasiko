//! Graceful degradation synthesis for Flow-Guard circuit breaker interventions.
//! Synthesizes valid OpenAI-compatible chat completion responses so agent harnesses
//! (Claude Code, OpenAI Assistants, LangGraph) remain intact instead of crashing on socket aborts.

use crate::cycle::CircuitBreakerTrip;
use serde_json::{json, Value};

/// Synthesizes a valid OpenAI JSON response payload when a circuit breaker trips.
pub struct GracefulDegradation;

impl GracefulDegradation {
    /// Formats a clean, harness-compliant completion demanding Human-In-The-Loop (HITL) intervention.
    pub fn synthesize_hitl_response(
        trip: &CircuitBreakerTrip,
        trace_id: &str,
        model: &str,
    ) -> Value {
        let reason_text = match trip {
            CircuitBreakerTrip::MaxDepthExceeded { depth, max } => {
                format!(
                    "Recursive delegation depth limit reached ({}/{} hops). Potential runaway agent delegation.",
                    depth, max
                )
            }
            CircuitBreakerTrip::AnomalousCycleDetected { pattern, repetitions } => {
                format!(
                    "Anomalous cyclic delegation loop detected! Sequence [{}] was invoked {} times consecutively.",
                    pattern.join(" -> "),
                    repetitions
                )
            }
            CircuitBreakerTrip::BudgetExceeded { spent_usd, max_usd } => {
                format!(
                    "Cumulative token budget exceeded (${:.2} / ${:.2} cap).",
                    spent_usd, max_usd
                )
            }
        };

        let content = format!(
            "⚠️ [NASIKO FLOW-GUARD INTERVENTION]: Circuit breaker tripped under root trace_id '{}'.\n\
            Reason: {}\n\n\
            To prevent runaway financial drain and infinite A2A ping-pong loops, automated tool execution has been safely paused.\n\
            Operator intervention required: Please review agent state or authorize continuation.",
            trace_id, reason_text
        );

        json!({
            "id": format!("chatcmpl-flowguard-{}", &trace_id[..trace_id.len().min(8)]),
            "object": "chat.completion",
            "created": chrono::Utc::now().timestamp(),
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": content
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 128,
                "completion_tokens": 64,
                "total_tokens": 192
            }
        })
    }
}
