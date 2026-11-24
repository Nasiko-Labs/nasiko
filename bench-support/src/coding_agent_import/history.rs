//! One user's synthetic coding-agent history: sessions spread over a window of
//! days, some of them subagents of another, plus a few live turns.
//!
//! Every turn is built from a seed and its (session, turn) position, so the same
//! options always produce the same event ids and payloads — a rerun against the
//! same database is a re-import, answered `duplicate` turn for turn. Turns are
//! built only when the client is about to send them; [`Expected`] accumulates
//! what the server must end up holding as they are.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use nasiko_types::coding_agent::CodingAgentSessionParent;
use nasiko_types::{
    CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1, CodingAgentLlmCall,
    CodingAgentSession, CodingAgentSource, CodingAgentTimestampQuality, CodingAgentToolAssociation,
    CodingAgentToolCall, CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_event_id,
    coding_agent_session_id,
};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

/// The coding-agent integration every synthetic user registers.
pub const INTEGRATION_ID: &str = "claude";
pub const PROVIDER: &str = "anthropic";
/// Parent sessions' model; subagents run on [`CHILD_MODEL`].
pub const MAIN_MODEL: &str = "claude-sonnet-4-5";
pub const CHILD_MODEL: &str = "claude-haiku-4-5";
pub const MODELS: [&str; 2] = [MAIN_MODEL, CHILD_MODEL];

/// About this many turns per top-level session (~200 sessions for 20,000 turns).
const TURNS_PER_SESSION: usize = 100;
/// Share of top-level sessions that spawn subagents, and how many each spawns.
const SPAWNING_SHARE: f64 = 0.25;
const MAX_CHILDREN: usize = 3;
/// Share of sessions captured with content (prompt, response, tool arguments).
const CONTENT_SHARE: f64 = 0.15;
/// History turns are at least this old: older than the server's live/history
/// split (1 h) and clear of today's price-book boundary.
const NEWEST_HISTORY: Duration = Duration::days(1);
/// Spacing between turn starts in a session; turns last less, so never overlap.
const TURN_SPACING_SECS: i64 = 240;
const LIVE_SPACING_SECS: i64 = 30;

/// Per-class token counts: input, output, cache read, cache creation.
pub type Tokens = [u64; 4];

#[derive(Debug, Clone, Copy)]
pub struct Shape {
    pub turns: usize,
    pub live_turns: usize,
    pub days: i64,
    pub seed: u64,
    /// Leading turns of one content session that carry ~1.2 MB of content each, so
    /// a batch of them is over the server's 8 MiB body limit and must be split.
    pub bulky_turns: usize,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct SessionPlan {
    source_id: String,
    parent: Option<usize>,
    start: DateTime<Utc>,
    turns: usize,
    content: bool,
    live: bool,
    /// Its first this-many turns are bulky.
    bulky: usize,
}

/// What the server must hold for this user once every turn is delivered.
#[derive(Debug, Clone, Default)]
pub struct Expected {
    pub turns: u64,
    pub tokens: u64,
    /// Token classes per (model, priced before the model's first price-book row).
    pub usage: HashMap<(String, bool), Tokens>,
    pub live_event_ids: Vec<String>,
}

/// When each model's first `model_pricing` row takes effect: a call before it is
/// priced from the static fallback book, after it from the database row.
pub type PriceBoundaries = HashMap<String, Option<DateTime<Utc>>>;

pub struct History {
    run_id: String,
    user: usize,
    agent_name: String,
    seed: u64,
    sessions: Vec<SessionPlan>,
    order: Vec<(u32, u32)>,
    cursor: usize,
    boundaries: PriceBoundaries,
    pub expected: Expected,
}

impl History {
    pub fn plan(
        run_id: &str,
        user: usize,
        agent_name: &str,
        shape: Shape,
        boundaries: PriceBoundaries,
    ) -> Self {
        let seed = mix(shape.seed, user as u64, u64::MAX, 0);
        let mut rng = StdRng::seed_from_u64(seed);
        let mut sessions = plan_sessions(run_id, user, shape, &mut rng);
        let mut order: Vec<(u32, u32)> = Vec::with_capacity(shape.turns);
        let mut indices: Vec<usize> = (0..sessions.len()).filter(|&s| !sessions[s].live).collect();
        indices.shuffle(&mut rng);
        for s in indices {
            order.extend((0..sessions[s].turns).map(|k| (s as u32, k as u32)));
        }
        if shape.live_turns > 0 {
            let live = sessions.len();
            sessions.push(SessionPlan {
                source_id: format!("lt-{run_id}-u{user}-live"),
                parent: None,
                start: shape.now - Duration::seconds(LIVE_SPACING_SECS * shape.live_turns as i64),
                turns: shape.live_turns,
                content: false,
                live: true,
                bulky: 0,
            });
            // Live turns arrive while the import is under way, as a Stop hook's would.
            let step = order.len() / (shape.live_turns + 1);
            for k in (0..shape.live_turns).rev() {
                order.insert(step * (k + 1), (live as u32, k as u32));
            }
        }
        Self {
            run_id: run_id.to_owned(),
            user,
            agent_name: agent_name.to_owned(),
            seed,
            sessions,
            order,
            cursor: 0,
            boundaries,
            expected: Expected::default(),
        }
    }

    pub fn total(&self) -> usize {
        self.order.len()
    }

    pub fn top_level_sessions(&self) -> usize {
        self.sessions.iter().filter(|s| s.parent.is_none()).count()
    }

    pub fn child_sessions(&self) -> usize {
        self.sessions.iter().filter(|s| s.parent.is_some()).count()
    }

    pub fn top_level_turns(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| s.parent.is_none())
            .map(|s| s.turns)
            .sum()
    }

    /// Every event id this history produces, without building the turns.
    pub fn event_ids(&self) -> impl Iterator<Item = String> + '_ {
        self.order.iter().map(|&(s, k)| {
            let source_id = &self.sessions[s as usize].source_id;
            coding_agent_event_id(INTEGRATION_ID, source_id, &turn_id(k as usize))
        })
    }

    /// The next `n` turns in send order, or fewer at the end.
    pub fn next_turns(&mut self, n: usize) -> Vec<CodingAgentEventV1> {
        let end = (self.cursor + n).min(self.order.len());
        let picked: Vec<(u32, u32)> = self.order[self.cursor..end].to_vec();
        self.cursor = end;
        picked
            .into_iter()
            .map(|(s, k)| {
                let event = self.build(s as usize, k as usize);
                self.account(s as usize, &event);
                event
            })
            .collect()
    }

    fn account(&mut self, session: usize, event: &CodingAgentEventV1) {
        self.expected.turns += 1;
        if self.sessions[session].live {
            self.expected.live_event_ids.push(event.event_id.clone());
        }
        for call in &event.turn.llm_calls {
            let boundary = self.boundaries.get(&call.model).copied().flatten();
            let before_book = boundary.is_some_and(|at| call.started_at < at);
            let counts = [
                call.input_tokens,
                call.output_tokens,
                call.cache_read_tokens,
                call.cache_creation_tokens,
            ];
            self.expected.tokens += counts.iter().sum::<u64>();
            let slot = self
                .expected
                .usage
                .entry((call.model.clone(), before_book))
                .or_default();
            for (total, count) in slot.iter_mut().zip(counts) {
                *total += count;
            }
        }
    }

    fn build(&self, session: usize, k: usize) -> CodingAgentEventV1 {
        let plan = &self.sessions[session];
        let mut rng = StdRng::seed_from_u64(mix(self.seed, session as u64, k as u64, 1));
        let (spacing, max_len) = if plan.live {
            (LIVE_SPACING_SECS, 20)
        } else {
            (TURN_SPACING_SECS, 180)
        };
        let started_at = plan.start + Duration::seconds(spacing * k as i64);
        let ended_at = started_at + Duration::seconds(rng.random_range(5..max_len));
        let calls = llm_calls(&mut rng, plan, k, started_at, ended_at);
        let policy = if plan.content {
            CapturePolicy::Content
        } else {
            CapturePolicy::MetadataOnly
        };
        let tools = tool_calls(&mut rng, &calls, k, plan.content);
        let parent = plan.parent.map(|p| CodingAgentSessionParent {
            source_id: self.sessions[p].source_id.clone(),
            tool_call_id: Some(format!("toolu_spawn_{}_{p}_{session}", self.user)),
            agent_type: Some("Explore".into()),
        });
        let content = plan.content.then(|| {
            if k < plan.bulky {
                bulky_text(k)
            } else {
                text(&mut rng, k)
            }
        });
        CodingAgentEventV1 {
            version: CODING_AGENT_EVENT_VERSION,
            event_id: coding_agent_event_id(INTEGRATION_ID, &plan.source_id, &turn_id(k)),
            captured_at: ended_at,
            source: CodingAgentSource {
                agent_id: INTEGRATION_ID.into(),
                agent_name: self.agent_name.clone(),
            },
            session: CodingAgentSession {
                id: coding_agent_session_id(INTEGRATION_ID, &plan.source_id),
                source_id: plan.source_id.clone(),
                title: None,
                parent,
                place: None,
            },
            turn: CodingAgentTurn {
                id: turn_id(k),
                prompt: content.clone().map(|c| format!("prompt: {c}")),
                response: content.map(|c| format!("response: {c}")),
                started_at,
                ended_at,
                llm_calls: calls,
                tool_calls: tools,
            },
            capture_policy: policy,
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

fn turn_id(k: usize) -> String {
    format!("turn-{k:06}")
}

/// Top-level sessions, a quarter of them with subagent children, every one with
/// at least one turn and the rest of the history spread over them.
fn plan_sessions(run_id: &str, user: usize, shape: Shape, rng: &mut StdRng) -> Vec<SessionPlan> {
    let history = shape.turns.saturating_sub(shape.live_turns);
    let parents = (history / TURNS_PER_SESSION).max(1).min(history.max(1));
    let mut sessions: Vec<SessionPlan> = (0..parents)
        .map(|i| session(run_id, user, i, None, rng))
        .collect();
    for p in 0..parents {
        if !rng.random_bool(SPAWNING_SHARE) {
            continue;
        }
        for _ in 0..rng.random_range(1..=MAX_CHILDREN) {
            if sessions.len() >= history {
                break;
            }
            let index = sessions.len();
            sessions.push(session(run_id, user, index, Some(p), rng));
        }
    }
    for plan in sessions.iter_mut() {
        plan.turns = 1;
    }
    // Subagents are shorter than the sessions that spawn them.
    let weights: Vec<u32> = sessions
        .iter()
        .map(|s| if s.parent.is_some() { 1 } else { 3 })
        .collect();
    let total_weight: u32 = weights.iter().sum();
    for _ in sessions.len()..history {
        let mut pick = rng.random_range(0..total_weight);
        let index = weights
            .iter()
            .position(|&w| {
                if pick < w {
                    return true;
                }
                pick -= w;
                false
            })
            .unwrap_or(0);
        sessions[index].turns += 1;
    }
    if shape.bulky_turns > 0 {
        let index = sessions.iter().position(|s| s.content).unwrap_or(0);
        sessions[index].content = true;
        sessions[index].bulky = shape.bulky_turns.min(sessions[index].turns);
    }
    place_in_time(&mut sessions, shape, rng);
    sessions
}

fn session(
    run_id: &str,
    user: usize,
    index: usize,
    parent: Option<usize>,
    rng: &mut StdRng,
) -> SessionPlan {
    SessionPlan {
        source_id: format!("lt-{run_id}-u{user}-s{index:04}"),
        parent,
        start: DateTime::<Utc>::MIN_UTC,
        turns: 0,
        content: rng.random_bool(CONTENT_SHARE),
        live: false,
        bulky: 0,
    }
}

/// Top-level sessions start anywhere in the window and end at least a day ago;
/// a subagent starts inside its parent's span.
fn place_in_time(sessions: &mut [SessionPlan], shape: Shape, rng: &mut StdRng) {
    let span = |turns: usize| Duration::seconds(TURN_SPACING_SECS * turns as i64);
    let newest = shape.now - NEWEST_HISTORY;
    let oldest = shape.now - Duration::days(shape.days);
    for plan in sessions.iter_mut().filter(|s| s.parent.is_none()) {
        let latest = newest - span(plan.turns);
        let room = (latest - oldest).num_seconds().max(1);
        plan.start = oldest + Duration::seconds(rng.random_range(0..room));
    }
    for i in 0..sessions.len() {
        let Some(p) = sessions[i].parent else {
            continue;
        };
        let parent = &sessions[p];
        let offset = rng.random_range(0..span(parent.turns).num_seconds().max(1));
        let start = parent.start + Duration::seconds(offset);
        // Never past the newest history: a long child is pulled back.
        let latest = newest - span(sessions[i].turns);
        sessions[i].start = start.min(latest);
    }
}

fn llm_calls(
    rng: &mut StdRng,
    plan: &SessionPlan,
    k: usize,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
) -> Vec<CodingAgentLlmCall> {
    let count = rng.random_range(1..=4_i64);
    let slice = (ended_at - started_at).num_milliseconds() / count;
    (0..count)
        .map(|i| {
            let model = if plan.parent.is_some() || rng.random_bool(0.05) {
                CHILD_MODEL
            } else {
                MAIN_MODEL
            };
            let call_start = started_at + Duration::milliseconds(slice * i);
            // Prompt (input + cache) stays under 200k: no long-context pricing tier.
            CodingAgentLlmCall {
                id: format!("msg_{k}_{i}"),
                provider: PROVIDER.into(),
                model: model.into(),
                input_tokens: rng.random_range(50..8_000),
                output_tokens: rng.random_range(20..4_000),
                cache_read_tokens: if rng.random_bool(0.8) {
                    rng.random_range(1_000..120_000)
                } else {
                    0
                },
                cache_creation_tokens: if rng.random_bool(0.5) {
                    rng.random_range(100..20_000)
                } else {
                    0
                },
                accounting: None,
                started_at: call_start,
                ended_at: call_start + Duration::milliseconds(slice),
            }
        })
        .collect()
}

fn tool_calls(
    rng: &mut StdRng,
    calls: &[CodingAgentLlmCall],
    k: usize,
    content: bool,
) -> Vec<CodingAgentToolCall> {
    const NAMES: [&str; 4] = ["Read", "Edit", "Bash", "Grep"];
    (0..rng.random_range(0..=3))
        .map(|i| {
            let call = &calls[rng.random_range(0..calls.len())];
            let failed = rng.random_bool(0.03);
            CodingAgentToolCall {
                id: format!("toolu_{k}_{i}"),
                name: NAMES[rng.random_range(0..NAMES.len())].into(),
                kind: "builtin".into(),
                model_call_id: Some(call.id.clone()),
                status: if failed {
                    CodingAgentToolCallStatus::Failed
                } else {
                    CodingAgentToolCallStatus::Succeeded
                },
                arguments: content.then(|| serde_json::json!({"path": format!("src/file_{i}.rs")})),
                output: content.then(|| serde_json::json!({"ok": !failed})),
                raw: None,
                error: (content && failed).then(|| "exit status 1".into()),
                started_at: Some(call.started_at),
                ended_at: Some(call.ended_at),
                duration_ms: Some(
                    (call.ended_at - call.started_at).num_milliseconds().max(0) as u64
                ),
                association: CodingAgentToolAssociation::Exact,
                timestamp_quality: CodingAgentTimestampQuality::Exact,
            }
        })
        .collect()
}

/// Bytes of filler in each of a bulky turn's prompt and response: under the
/// 1 MiB field limit, so the turn is valid, but eight of them exceed 8 MiB.
const BULKY_FIELD_BYTES: usize = 600 * 1024;

/// A pasted log's worth of content.
fn bulky_text(k: usize) -> String {
    let line = format!("turn {k}: 2026-10-21T08:00:00Z INFO build step finished ok\n");
    line.repeat(BULKY_FIELD_BYTES / line.len())
}

/// A few hundred bytes to a couple of KB of filler content.
fn text(rng: &mut StdRng, k: usize) -> String {
    let words = rng.random_range(30..300);
    let mut out = format!("turn {k}:");
    for i in 0..words {
        out.push(' ');
        out.push_str(
            [
                "refactor", "the", "parser", "and", "add", "tests", "for", "edge",
            ][i % 8],
        );
    }
    out
}

/// A well-spread seed from a few coordinates (splitmix64 rounds).
fn mix(seed: u64, a: u64, b: u64, c: u64) -> u64 {
    let mut x = seed;
    for v in [a, b, c] {
        x ^= v.wrapping_add(0x9e37_79b9_7f4a_7c15);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^= x >> 31;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(turns: usize) -> Shape {
        Shape {
            turns,
            live_turns: 3,
            days: 90,
            seed: 7,
            bulky_turns: 4,
            now: Utc::now(),
        }
    }

    #[test]
    fn every_generated_turn_is_valid_and_ids_are_unique() {
        let mut history = History::plan("t", 0, "u0-claude-code", shape(2_000), HashMap::new());
        let ids: std::collections::HashSet<String> = history.event_ids().collect();
        assert_eq!(ids.len(), 2_000);
        let mut built = 0;
        loop {
            let turns = history.next_turns(100);
            if turns.is_empty() {
                break;
            }
            for event in &turns {
                event
                    .validate()
                    .unwrap_or_else(|e| panic!("{e}: {event:?}"));
                assert!(ids.contains(&event.event_id));
            }
            built += turns.len();
        }
        assert_eq!(built, 2_000);
        assert_eq!(history.expected.turns, 2_000);
        assert_eq!(history.expected.live_event_ids.len(), 3);
        assert!(
            history.child_sessions() > 0,
            "some sessions spawn subagents"
        );
    }

    #[test]
    fn the_same_seed_rebuilds_identical_turns() {
        let now = Utc::now();
        let shape = Shape { now, ..shape(500) };
        let mut a = History::plan("t", 1, "n", shape, HashMap::new());
        let mut b = History::plan("t", 1, "n", shape, HashMap::new());
        assert_eq!(a.next_turns(500), b.next_turns(500));
    }

    #[test]
    fn history_stays_inside_the_window_and_a_day_old() {
        let now = Utc::now();
        let mut history = History::plan(
            "t",
            2,
            "n",
            Shape {
                now,
                ..shape(1_000)
            },
            HashMap::new(),
        );
        for event in history.next_turns(1_000) {
            let live = history.expected.live_event_ids.contains(&event.event_id);
            if live {
                assert!(event.turn.ended_at <= now);
                assert!(event.turn.started_at > now - Duration::hours(1));
            } else {
                assert!(event.turn.ended_at <= now - NEWEST_HISTORY);
                assert!(event.turn.started_at >= now - Duration::days(90));
            }
        }
    }
}
