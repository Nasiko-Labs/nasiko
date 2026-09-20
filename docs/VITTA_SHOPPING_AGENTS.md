# Vitta: a spending-policy gate and four shopping agents on Nasiko

Vitta is an application that lets AI agents shop on a person's behalf **without being able to spend more
than the person allowed**. This document specifies what Vitta implements and how it uses Nasiko, and it
records what running it against a real Nasiko turned up. Source: <https://github.com/Agnik47/Vitta>.

> **Status of this document.** It describes Vitta's current working tree. The public branch
> (`feature/razorpay-test-mode`) is behind it: the decision log, the review hand-off, the post-purchase cart
> emptying, the browser queue, the cross-platform work and the Planner/Evaluator fixes listed below are
> implemented and tested locally and are being pushed. Every claim is marked **verified** (run against the real
> thing), **tested** (automated tests only) or **not verified**.

## 1. The boundary: who decides what

| | Responsibility |
|---|---|
| **Nasiko** | Agent registry and identity, routing (`/api/orchestrator/a2a`), per-hop traces, agent lifecycle. **Not a policy engine.** |
| **Vitta gate** | The only financial authority. A deterministic, synchronous, LLM-free `decide()` over a signed mandate. |
| **Anakin** | The agents' access to the web (product search), through the dashboard's Anakin-first source. |
| **Razorpay (test mode)** | The order that is the funded reserve the gate draws from. |
| **The agents** | Propose. They never hold payment credentials and cannot spend. |

Hard rules the code enforces: **test-mode payments only** (a live Razorpay key is refused before any request);
a `DENY` stops the browser action; payment secrets stay server-side and never reach the browser.

## 2. Feature specification

### 2.1 Mandates: signed spending authority
- An **Ed25519-signed** mandate names the subject, the allowed **merchants**, a **per-transaction cap**, a
  **total cap**, a **maximum number of transactions**, an **expiry**, and the **reserve** it is funded from.
- Created and re-signed through the `gate` CLI (`gate mandate create|resign`) and from the dashboard, which only
  ever spawns the CLI. Signature and expiry are checked on every action. **Tested.**

### 2.2 The policy gate
- Every governed action goes through `gate run -- webcmd <site> <command>`. The gate prices the **real merchant
  cart** (read live through `webcmd`), then decides. Reads are free; writes are decided.
- Refusals are typed: `BAD_SIGNATURE`, `EXPIRED`, `UNKNOWN_COMMAND`, `MERCHANT_NOT_ALLOWED`,
  `AMOUNT_UNPARSEABLE`, `OVER_PER_TXN_CAP`, `OVER_TOTAL_CAP`, `TXN_LIMIT_REACHED`, `CART_DRIFT`,
  `INSUFFICIENT_RESERVE`, `ALREADY_EXECUTED`. An exhausted mandate refuses **every** write, even a ₹0 one.
- Two execution modes, always explicit: **TEST** settles against the Razorpay test reserve and signs a receipt
  without driving the merchant's checkout; **LIVE** drives checkout and fails closed if the merchant never
  confirms an order. **Tested**; TEST purchases **verified** end to end against the real Razorpay test API. LIVE
  merchant checkout is **not verified**.

### 2.3 The reserve ledger (Razorpay test mode)
- A Razorpay **Order is the reserve**. Balance = captured payments, net of refunds, minus draws. Draws are recorded
  twice (a local append-only ledger and the order's server-side notes) and the larger figure wins, so losing the
  local file cannot un-spend money. Draws are idempotent on the run id; a top-up is refused; one order belongs to
  exactly one mandate; an unpaid or foreign order is never attached.
- Funding: Razorpay Checkout in the dashboard, with signature verification (`HMAC-SHA256(order_id|payment_id)`),
  an HMAC-checked webhook deduplicated by event id, and a hosted pay page. **Verified** against the real test API
  (order creation, refusal while unpaid, a real Visa test payment, real draws, replay and overdraw refusals).
  Real webhook delivery is **not verified** (it needs a public tunnel).

### 2.4 Receipts and the decision log
- **Spend receipts**: signed, hash-chained, one per allowed spend; tampering with one breaks the next link.
- **Funding receipts**: one signed receipt per Razorpay order that funded a mandate, built from Razorpay's own
  payment records; a re-attach never re-issues it.
- **Transaction authorizations**: signed evidence that the mandate approved a spend, written before the merchant confirms.
- **Decision log** (`events.jsonl`): the gate's ALLOW/DENY/STEP_UP verdicts **and** activity entries: mandate
  created, payment order created / received, purchase completed / failed, cart emptied, each agent run, and any
  command that failed before it could reach a decision. Best-effort by design: recording never breaks the action.

### 2.5 The four agents
| Agent | Does | Notes |
|---|---|---|
| **Shopping Planner** | Turns "Help me in buying 1L milk under 70" into a structured intent (product, quantity, price ceiling, merchants, buy or search) | Deterministic parser, no LLM. Peels stacked conversational lead-ins; recognises *buy/buying/order/purchasing*. |
| **Deal Discovery** | Finds candidates across Blinkit, Zepto and BigBasket | Anakin first, `webcmd` fallback; each merchant search has its own 50 s deadline; the merchant's stated pack size (Blinkit `variant`, Zepto `pack_size`) is carried into the name. |
| **Deal Evaluator** | Picks the cheapest eligible candidate | Deterministic: relevance to the query, a stated pack size that matches the one asked for (never guessed), a price ceiling, in stock, merchant scope. Explains every rejection. |
| **Purchase Agent** | Buys the pick, only through the gate | Idempotent on the request id (a replayed request never buys twice); returns the gate's refusal verbatim. |

### 2.6 Human in the loop
- The default run **never enters the Purchase Agent**. It ends *Ready for your review* with the pick offered as
  *Add to cart & review*; the person reviews it in the Cart and presses *Proceed to purchase*, which is the
  existing gated purchase path.
- An autonomous purchase is an explicit, confirmed opt-in ("Let the agents place the order for me"); the mandate
  and gate still decide.

### 2.7 The real cart
- The Cart page mirrors the merchant's **real** cart, never a local copy. Blinkit writes are **absolute**
  (`set-cart-quantity`), so a retry or a double-click cannot multiply a quantity; every write is followed by a
  verifying read.
- After a completed purchase the cart is **emptied and confirmed empty by a real read**, without ever undoing the
  purchase. A cart the gate will not let it empty (exhausted mandate) is reported, not hidden.
- A guest-cart notice appears when the shopping browser is not signed in to the merchant.

### 2.8 Price Sniper
Watches one real Blinkit product in a time window and fires the same purchase pipeline the first time it is at or
below a target. While waiting, it takes read-only **preview** readings so the page shows a real last-seen price and
check count; a preview can never trigger a purchase.

### 2.9 Dashboard
Search & compare, Cart, Price sniper, Agent activity (per-hop timeline, verdict shown separately from the agents,
Nasiko trace panel), Mandate (create, fund, inspect), Decision log, Receipts (spend and funding), Docs.

### 2.10 Reliability and portability
- One shared `webcmd` browser session is driven **one call at a time per site**; a "session busy" refusal is waited
  out; a hung session is reset once, and only for operations that are safe to repeat. A purchase is never retried
  after it may have started.
- Runs on macOS, Linux and Windows (Node 20+): `.env` parsing tolerates a BOM, CRLF and quotes; `.gitattributes`
  pins LF; the deploy script is Node (no bash/python/curl/zip); a CI matrix covers all three systems. **Verified**
  locally on macOS only; the Linux and Windows results come from CI.

## 3. How Vitta uses Nasiko

- Each agent is a standalone A2A server (defaults 9101–9104) with an `AgentCard.json`, packaged from one codebase by
  a Dockerfile taking `AGENT=<planner|discovery|evaluator|purchase>`.
- **Deploy without the Rust CLI**: `node nasiko/deploy.js <agent|all> --upload` zips each staged project and posts it
  to `POST /api/import/upload`, the language-agnostic import (`/api/agents/upload` insists on a Python `main.py`).
  Re-running an existing agent redeploys it as the next patch version, because Nasiko refuses to re-import a version
  it has seen. **Verified**: Planner, Discovery and Evaluator built and ran on a local Nasiko.
- **Routing**: a run is dispatched hop by hop through `POST /api/orchestrator/a2a`, one W3C `traceparent` per hop under
  one trace id. The Purchase Agent is called directly, beside the gate, because it needs the gate's local state.
- **Observability**: each stage stores the id Nasiko announces in the dispatch stream's `trace_meta` event, and
  `GET /api/observability/trace/<id>` resolved each to an `a2a.dispatch` span. The dashboard shows only what Nasiko
  returns and invents nothing.

## 4. Notes from running against a real Nasiko

These were found by running it, not read from the docs; each is pinned by a test on the Vitta side. They may be
useful documentation upstream.

1. The orchestrator only deserializes the A2A v1 role enum, **`ROLE_USER`**, and answers 400 to `user`.
2. Nasiko calls agents with the v1 method names: **`SendStreamingMessage` first, then `SendMessage`** when the agent
   answers with a JSON-RPC error. An agent serving only `message/send` fails both attempts and Nasiko relays the
   placeholder "No response".
3. The orchestrator **always replies as an event stream** (`text/event-stream`), even for a plain send; the answer
   is in the `artifactUpdate` events.
4. Nasiko's shared HTTP client cuts every agent call off at **60 seconds**; Vitta's Discovery therefore gives each
   merchant search its own 50 s deadline, so one slow merchant is a recorded failure, not a dead hop.
5. **Secrets reach a container only when it is deployed** (the container does not read the caller's `.env`). A value
   such as the dashboard URL that Discovery calls is baked in at deploy time; a stale value silently starves the agent
   of whatever a newer dashboard build provides. Vitta documents this and redeploys to change it.
6. Nasiko's trace view is keyed by **its own trace id** (from `trace_meta`), not by the caller's `traceparent`.
7. Agent → agent calls are default-deny and the open-source edition exposes no endpoint to grant them, so the
   orchestrator calls each agent itself.

## 5. Verification

- **734 automated tests** (`node:test`, no external runner), including end-to-end suites that run the real gate CLI
  against a Razorpay mock and a simulated merchant, plus five dependency-free check scripts and 24 dashboard route
  checks. A CI matrix runs on Ubuntu, macOS and Windows across Node 20, 22 and 24 (**not yet run on GitHub**).
- **Verified live:** Razorpay test-mode funding and draws; Anakin search; Planner, Discovery and Evaluator running on a
  real local Nasiko (Planner v0.1.3, Discovery v0.1.4, Evaluator v0.1.2) with per-hop traces; real Blinkit cart
  reads and writes through `webcmd`; TEST-mode purchases with signed receipts from the dashboard's Cart.
- **Not verified:** a LIVE merchant checkout; real Razorpay webhook delivery; Vitta's own OpenTelemetry spans beyond
  Nasiko's per-hop span; the Rust `nasiko` CLI deploy path; Zepto search, which merchants' bot protection blocks
  intermittently, and BigBasket, where Anakin's scraper sometimes renders an empty page.

## 6. Scope of this PR

Documentation only: this file. No Nasiko code changes, no vendored agent code (the Purchase Agent needs a local gate
and merchant sessions, so it is not a self-contained example for `agents/`). The source and its deploy script live in
the Vitta repository.
