//! Stateful cycle and anomalous loop detector for autonomous A2A (Agent-to-Agent) communication.
//! Grouped by root trace_id, tracks depth, fan-out, token velocity, and repetitive tool call sequences.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

/// Thresholds for autonomous A2A flow safety.
#[derive(Debug, Clone)]
pub struct FlowGuardConfig {
    /// Maximum recursive call depth between agents (default: 5)
    pub max_depth: usize,
    /// Maximum identical sequence repetitions before emergency trip (default: 3)
    pub cycle_repetition_threshold: usize,
    /// Maximum allowable dollar cost per root trace (default: $2.00)
    pub max_budget_usd: f64,
    /// Sliding window size for tool call sequence hashing (default: 10)
    pub sliding_window_size: usize,
}

impl Default for FlowGuardConfig {
    fn default() -> Self {
        Self {
            max_depth: 5,
            cycle_repetition_threshold: 3,
            max_budget_usd: 2.00,
            sliding_window_size: 10,
        }
    }
}

/// A recorded tool invocation in an A2A sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolCallRecord {
    pub caller_agent: String,
    pub target_agent: String,
    pub tool_name: String,
}

/// Real-time telemetry for an active root trace.
#[derive(Debug, Clone)]
pub struct TraceTelemetry {
    pub trace_id: String,
    pub current_depth: usize,
    pub cumulative_tokens: u64,
    pub cumulative_cost_usd: f64,
    pub recent_tools: VecDeque<ToolCallRecord>,
    pub started_at: Instant,
}

/// Interception reason when FlowGuard acts as an emergency brake.
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitBreakerTrip {
    /// Depth exceeded limit (e.g. 5 hops)
    MaxDepthExceeded { depth: usize, max: usize },
    /// Identical sequence of tools was repeated N times in a row
    AnomalousCycleDetected {
        pattern: Vec<String>,
        repetitions: usize,
    },
    /// Rapid token expenditure exceeded budget cap
    BudgetExceeded {
        spent_usd: f64,
        max_usd: f64,
    },
}

/// In-memory stateful flow-guard tracking A2A communication.
pub struct StatefulFlowGuard {
    config: FlowGuardConfig,
    active_traces: HashMap<String, TraceTelemetry>,
}

impl StatefulFlowGuard {
    pub fn new(config: FlowGuardConfig) -> Self {
        Self {
            config,
            active_traces: HashMap::new(),
        }
    }

    /// Record an agent turn and check if the circuit breaker should trip.
    pub fn inspect_and_record(
        &mut self,
        trace_id: &str,
        caller: &str,
        target: &str,
        tool_name: &str,
        tokens: u64,
        cost_usd: f64,
    ) -> Result<(), CircuitBreakerTrip> {
        let telemetry = self.active_traces.entry(trace_id.to_string()).or_insert_with(|| {
            TraceTelemetry {
                trace_id: trace_id.to_string(),
                current_depth: 0,
                cumulative_tokens: 0,
                cumulative_cost_usd: 0.0,
                recent_tools: VecDeque::new(),
                started_at: Instant::now(),
            }
        });

        // 1. Increment depth & costs
        telemetry.current_depth += 1;
        telemetry.cumulative_tokens += tokens;
        telemetry.cumulative_cost_usd += cost_usd;

        let record = ToolCallRecord {
            caller_agent: caller.to_string(),
            target_agent: target.to_string(),
            tool_name: tool_name.to_string(),
        };

        telemetry.recent_tools.push_back(record);
        if telemetry.recent_tools.len() > self.config.sliding_window_size {
            telemetry.recent_tools.pop_front();
        }

        // 2. Check depth ceiling
        if telemetry.current_depth > self.config.max_depth {
            return Err(CircuitBreakerTrip::MaxDepthExceeded {
                depth: telemetry.current_depth,
                max: self.config.max_depth,
            });
        }

        // 3. Check budget ceiling
        if telemetry.cumulative_cost_usd > self.config.max_budget_usd {
            return Err(CircuitBreakerTrip::BudgetExceeded {
                spent_usd: telemetry.cumulative_cost_usd,
                max_usd: self.config.max_budget_usd,
            });
        }

        // 4. Detect anomalous cyclic patterns in the sliding window
        if let Some((pattern, reps)) = self.detect_cycle(&telemetry.recent_tools) {
            if reps >= self.config.cycle_repetition_threshold {
                return Err(CircuitBreakerTrip::AnomalousCycleDetected {
                    pattern,
                    repetitions: reps,
                });
            }
        }

        Ok(())
    }

    /// Detect if the end of the window consists of a repeating pattern of length 1, 2, or 3.
    fn detect_cycle(&self, window: &VecDeque<ToolCallRecord>) -> Option<(Vec<String>, usize)> {
        let items: Vec<String> = window.iter().map(|t| t.tool_name.clone()).collect();
        let n = items.len();

        for len in 1..=3 {
            if n < len * 2 {
                continue;
            }
            let pattern = &items[n - len..];
            let mut reps = 1;
            let mut i = n - len;

            while i >= len {
                if &items[i - len..i] == pattern {
                    reps += 1;
                    i -= len;
                } else {
                    break;
                }
            }

            if reps >= self.config.cycle_repetition_threshold {
                return Some((pattern.to_vec(), reps));
            }
        }

        None
    }
}
