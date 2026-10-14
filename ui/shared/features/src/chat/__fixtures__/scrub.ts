// Ported from nasiko-ui-lab@1b99bdda scripts/smoke-chat-routed.ts (its scrub: `KEEP`, `KEY_SHAPE`, `PLACEHOLDER`,
// `scrub`) and scripts/lib/live.ts (`SECRET`). Local edits: only the pieces live-routed.fixture.test.ts checks the
// recorded fixture against, beside the fixture (the live smoke and recorder scripts themselves are lab tooling, not
// ported); `map.get(v)!` reads through a local; Prettier formatting.
/**
 * The only string fields the fixture keeps verbatim (an allowlist: an unknown field is scrubbed,
 * so a new server field can't leak agent or LLM text). Frame and part discriminators, agent,
 * tool and model names, enums, timestamps, and ids (which `scrub` also swaps for placeholders).
 * Mirrored by src/features/chat/live-routed.fixture.test.ts.
 */
export const KEEP = new Set([
  // Discriminators and enums.
  "type",
  "kind",
  "state",
  "role",
  "method",
  "jsonrpc",
  "status",
  "success",
  "code",
  // Names.
  "agent",
  "via_agent",
  "agent_name",
  "caller_agent_name",
  "tool",
  "model",
  // Ids (placeholders by the time they're kept) and times.
  "id",
  "taskId",
  "contextId",
  "messageId",
  "artifactId",
  "task_id",
  "context_id",
  "trace_id",
  "traceId",
  "session_id",
  "sessionId",
  "span_id",
  "timestamp",
]);

/** Object keys kept as they are: short identifiers. Anything else becomes `k-<n>`. */
export const KEY_SHAPE = /^(?:[A-Za-z_][A-Za-z0-9_]{0,39}|k-\d+)$/;
/** The placeholders `scrub` writes: ids, and the length class of a scrubbed string. */
export const PLACEHOLDER = /^(?:(?:id|hex)-\d+|session-1|trace-1|<[\w-]+ \d+ chars>)$/;

/** Likely credentials; the fixture writer refuses to write any (a second line of defence). Shared with the recorder. */
export const SECRET =
  /eyJ[\w-]{10,}|\bsk-[\w-]{16,}|\bghp_\w{20,}|\bAKIA[0-9A-Z]{16}\b|Bearer\s+[\w.-]{16,}|-----BEGIN [A-Z ]*PRIVATE KEY-----/;

/**
 * The fixture keeps the stream's shape, never its content (NE-6): ids become stable
 * placeholders, strings under a `KEEP` key stay (capped at 200 chars), and every other
 * non-empty string becomes `<key N chars>`. Numbers and booleans stay.
 */
export function scrub(
  frames: unknown[],
  ids: { sessionId: string; traceId: string | null },
): unknown[] {
  const map = new Map<string, string>();
  const place = (v: string, kind: string) => {
    const known = map.get(v);
    if (known) return known;
    const next = `${kind}-${map.size + 1}`;
    map.set(v, next);
    return next;
  };
  if (ids.traceId) map.set(ids.traceId, "trace-1");
  map.set(ids.sessionId, "session-1");
  const cut = (s: string) => (s.length > 200 ? `${s.slice(0, 200)}…` : s);
  // Keys are data too (an agent's data part can use free text as a key): keep identifier-shaped ones.
  let keys = 0;
  const scrubKey = (k: string) => (KEY_SHAPE.test(k) ? k : `k-${++keys}`);
  const walk = (v: unknown, key = ""): unknown => {
    if (typeof v === "string") {
      let s = v;
      for (const [real, fake] of map) s = s.split(real).join(fake);
      s = s
        .replace(/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi, (m) =>
          place(m, "id"),
        )
        .replace(/\b[0-9a-f]{32}\b/gi, (m) => place(m, "hex"));
      if (KEEP.has(key) || !s) return cut(s);
      return `<${key || "value"} ${s.length} chars>`;
    }
    if (Array.isArray(v)) return v.map((x) => walk(x, key));
    if (v && typeof v === "object")
      return Object.fromEntries(
        Object.entries(v)
          .filter(([k]) => !/^(authorization|cookie|set-cookie|token)$/i.test(k))
          .map(([k, x]) => {
            const key = scrubKey(k);
            return [key, walk(x, key)];
          }),
      );
    return v;
  };
  return frames.map((f) => walk(f));
}
