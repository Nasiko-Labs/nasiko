/**
 * Per-message usage chips: "~1.2k tokens · 3.4s · $0.0021".
 *
 * Fed by the stream's terminal `usage_meta` data part, or by the equivalent
 * columns on a reloaded chat_messages row. Token/cost chips only exist for
 * platform-paid usage; a bring-your-own-key agent reply shows duration alone.
 * `estimated: true` (streamed orchestrator turns) prefixes figures with `~`.
 */

/** Normalize a chat_messages row into the usage_meta shape. */
export function usageFromMessage(m) {
  if (!m) return null;
  const hasTokens = m.input_tokens != null || m.output_tokens != null;
  if (!hasTokens && m.duration_ms == null) return null;
  return {
    input_tokens: m.input_tokens ?? 0,
    output_tokens: m.output_tokens ?? 0,
    total_tokens: hasTokens ? (m.input_tokens ?? 0) + (m.output_tokens ?? 0) : undefined,
    cost_usd: m.cost_usd,
    duration_ms: m.duration_ms,
    model: m.model,
    estimated: m.usage_estimated ?? false,
  };
}

export function usageChipsHtml(u) {
  if (!u) return "";
  const chips = [];
  const approx = u.estimated ? "~" : "";
  const total = u.total_tokens ?? 0;
  if (total > 0) chips.push(`${approx}${formatTokens(total)} tokens`);
  if (u.duration_ms != null) chips.push(formatDuration(u.duration_ms));
  const cost = toNumber(u.cost_usd);
  if (total > 0 && cost != null && cost > 0) chips.push(`${approx}${formatCost(cost)}`);
  if (!chips.length) return "";
  return `<span class="msg-usage" title="${escapeAttr(usageTitle(u))}">${chips.join(" · ")}</span>`;
}

function usageTitle(u) {
  const parts = [];
  if (u.total_tokens > 0) parts.push(`${u.input_tokens} in / ${u.output_tokens} out`);
  if (u.model) parts.push(u.model);
  if (u.estimated) parts.push("token counts are estimated");
  return parts.join(" · ");
}

function formatTokens(n) {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

function formatDuration(ms) {
  if (ms >= 60_000) return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${ms}ms`;
}

function formatCost(usd) {
  if (usd >= 0.01) return `$${usd.toFixed(2)}`;
  return `$${usd.toFixed(4)}`;
}

function toNumber(v) {
  if (v == null) return null;
  const n = typeof v === "number" ? v : parseFloat(v);
  return Number.isFinite(n) ? n : null;
}

function escapeAttr(s) {
  return String(s).replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;");
}
