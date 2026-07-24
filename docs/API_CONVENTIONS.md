# API Conventions — Target Standard

How every Nasiko HTTP endpoint should look on the wire. This is the third leg of the standards
set: `docs/CLEAN_CODE_GUIDE.md` governs how we write code, `docs/ORGANIZATION.md` governs where
it lives, and this doc governs what our requests and responses look like. Like the organization
standard, this is the **target**: follow it for new endpoints, and refactor toward it when you
touch existing ones.

**Scope.** Applies to every route the server exposes under `/api`. Protocol-owned surfaces —
A2A JSON-RPC, MCP JSON-RPC, the OCI Distribution registry at `/v2/*`, OIDC — follow their own
specs, not this document; where a protocol dictates a shape, the protocol wins.

---

## 1. Response envelopes — exactly two shapes

```jsonc
// single object
{ "data": { … } }

// list — always cursor-paged, even if the first version returns everything
{ "data": [ … ], "has_more": false, "next_cursor": null, "prev_cursor": null }
```

- Never a bare top-level array or raw row; never transport metadata (`status_code`,
  `message`) inside the body — the HTTP layer already says that.
- One envelope means one client-side deserializer for "an object" and one for "a list";
  every deviation multiplies integrator code.
- If a count is genuinely needed it is named `total_count` and is always the **full filtered
  count**, never the size of the returned page.
- Lists paginate by **cursor** from day one: `next_cursor`/`prev_cursor` are opaque strings,
  `has_more` says whether to keep going. No offset pagination on new endpoints — offsets skew
  under concurrent writes and can't be indexed efficiently.

## 2. Errors — one shape

```jsonc
{ "error": "human-readable message", "code": "machine_readable_slug" }
```

- Always JSON. Never plain text, never an empty body (a 204 is the only bodyless response).
- `code` is a stable slug clients switch on (`invalid_role`, `session_owned`, `last_admin`);
  the message may be reworded freely, the code may not change once shipped.
- 5xx bodies never carry internal error text — raw `e.to_string()` is a log line, not a
  response. The client gets the slug; the operator gets the detail in the logs, joined by
  the trace id.
- 429 responses carry a `Retry-After` header.

## 3. Status codes — the HTTP status is the only status

| Code | Meaning here |
|---|---|
| 200 | read, update, or idempotent re-create ("already exists and is yours") |
| 201 | resource created |
| 202 | accepted for async processing (build/deploy jobs) — body carries the job/stream handle |
| 204 | success with nothing to return (deletes, actions) |
| 400 | request understood but invalid (validation, bad reference) |
| 401 | not authenticated / stale credential |
| 403 | authenticated but not allowed |
| 404 | doesn't exist — **or deliberately hidden** (anti-enumeration; see below) |
| 409 | conflict with current state (duplicate name, last admin) |
| 413 | payload too large (every upload route states its limit) |
| 422 | body unparseable as the expected type |
| 429 | rate or flow limit hit — the `code` slug names which limit |

- Resource-existence probing is a real attack surface: where hiding existence matters (agent
  access), "no access" returns the **same 404** as "no such thing", consistently, and the
  route doc says so.
- Cascade/flow-limit rejections are 429s with a distinguishing `code` — they are "slow down /
  reduce fan-out" conditions, and clients should get to handle them with the same machinery
  as rate limits.

## 4. Naming & identifiers

- JSON fields are `snake_case`. Protocol surfaces keep their spec's casing (A2A is camelCase).
- Timestamps are RFC 3339 UTC. Durations carry their unit in the name (`latency_ms`,
  `expires_in` seconds).
- Every resource has one **canonical id: the UUID**. Accepting a human-readable name is an
  explicit, documented convenience on a per-route basis — never an accident of parsing — and
  the route doc states which forms it takes.
- The same field name means the same thing everywhere (the wire-format side of the clean-code
  "one word per concept" rule): if `total_count` is the filtered total on one route, it is on
  all of them.

## 5. The edition rule — wire-format twin of the trait-seam rule

**The enterprise edition may add fields to a response. It may never remove, rename, or re-shape them.**

The same route must return the same shape in both editions, so code developed against the
open-source edition runs unmodified against an enterprise deployment. An enterprise handler that
wants a different shape is the same smell as a fork of base logic — the fix is a seam in the base
handler, not a divergent response.

## 6. Streaming (SSE)

- Every streamed route documents its event vocabulary — the full set of event and data-part
  types it can emit — in the route doc.
- Events carry an SSE `id:`; where replay is feasible the route honors `Last-Event-ID` on
  reconnect. Where replay isn't feasible, the route provides a state-listing endpoint the
  client can re-read after a drop (the "pending list" pattern) — a dropped stream must never
  strand the client with no way to recover.
- Streams end with an explicit terminal event; clients never have to infer completion from
  the connection closing.
- Clients must be able to ignore unknown event types and data-part types — evolution of a
  stream vocabulary is additive.

## 7. Evolution & versioning

- There is no `/api/v1` path prefix; the contract evolves **additively**. New fields appear
  alongside old ones; nothing is removed or re-typed in place.
- A genuinely breaking change ships dual-shape behind a deprecation window, and a
  path-versioned break is the last resort — a deliberate team decision, never a side effect.
- Idempotency: mutations that external systems retry (create-user, deploys) accept a
  client-supplied identifier or natural key so a retry converges instead of duplicating.

## 8. Enforcement — types, not memory

- The serde-only `dto` crate (`docs/ORGANIZATION.md` §4) owns the envelope types:
  `Data<T>`, `Page<T>`, `ApiError`. Handlers return these types; the convention is an import,
  not a reviewer's recollection.
- Review checklist addition: *responses use the shared `dto` envelope types; errors are
  `ApiError` with a stable `code`; the enterprise edition adds fields, never re-shapes.*
- The OpenAPI spec is generated from the `dto` crate, so the published contract and the
  compiled one cannot drift.
