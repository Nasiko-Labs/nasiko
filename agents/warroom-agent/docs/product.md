# Product Concept & Requirements

> **WARROOM: Competitive intelligence grounded in live evidence and mapped to business reality.**

---

## 1. Problem Statement

Modern competitive strategy teams suffer not from a lack of information, but from an overwhelming torrent of unstructured noise:
- Product release notes and blogs buried on competitor sites
- Sudden pricing or tier alterations
- Subtly shifted marketing and homepage positioning
- Industry news and regulatory announcements

Traditional solutions either require tedious manual browsing across dozens of websites or rely on generic LLM chatbots that hallucinate outdated figures, invent pricing tiers, or generate bland, ungrounded advice.

The fundamental breakdown occurs between **information retrieval** and **strategic execution**:
1. **Raw data is not a signal**: An announcement about a fraud prevention tool is irrelevant unless linked to whether your own product line competes in that space.
2. **Generic summaries lack business context**: Knowing a competitor launched a feature does not inform whether your sales team needs a new battlecard or whether your product team needs an urgent roadmap review.
3. **Hallucination destroys trust**: Strategy teams cannot make multi-million-dollar decisions or advise enterprise prospects using fabricated claims or unsubstantiated statistics.

---

## 2. The WARROOM Concept

WARROOM addresses this challenge through a four-stage pipeline:

```text
┌─────────────────────────────────────────────────────────────┐
│ 1. OBSERVE (Live Web Retrieval)                             │
│ Gather current, verified public competitor updates via      │
│ targeted web search. Capture exact URLs and snippets.       │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 2. UNDERSTAND (Evidence-Grounded Extraction)                │
│ Filter noise. Identify what actually changed and classify   │
│ the change (product launch, pricing, market expansion).     │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 3. ASSESS (Company Context Overlay)                         │
│ Compare the competitor's movement against YOUR specific     │
│ product portfolio and customer segments. Determine overlap. │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ 4. RESPOND (Actionable Recommendations)                     │
│ Generate concrete takeaways for Product, Sales, and         │
│ Marketing—backed by direct links to source evidence.        │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. Target Users

| Persona | Core Need | WARROOM Value |
| :--- | :--- | :--- |
| **Product Leaders (CPO, VP Product, PMs)** | Early warning of rival feature launches and roadmap gaps | Immediate detection of rival product releases with direct impact assessments on matching internal features |
| **Sales Leaders & Account Executives** | Actionable objection handling and counter-positioning | Battlecards explaining what to say when a prospect brings up a competitor's newly launched solution |
| **Strategy & Market Intelligence Teams** | Synthesis of competitor strategic posture without manual daily scanning | Consolidated signal feed backed by verified web sources and timestamps |
| **SaaS Founders & Operators** | High-level situational awareness across key rivals | Rapid on-demand scan of market movements before executive reviews or investor updates |

---

## 4. Demo Company Profile: PayFlow

To anchor intelligence in realistic business reality, WARROOM operates with a default benchmark profile defined in [`data/company_context.json`](file:///home/Krishna-Singh/WarRoom/data/company_context.json):

- **Company Name**: `PayFlow`
- **Internal Product Portfolio**:
  1. `Payment Gateway` (Core transaction processing for web & mobile)
  2. `Reconciliation` (Automated settlement and ledger matching)
  3. `FraudShield` (Real-time risk scoring and payment fraud protection)
- **Target Customer Segments**:
  - `SMB` (Small and Medium Businesses)
  - `Mid-Market`
- **Monitored Competitors**:
  - `Cashfree`
  - `Razorpay`
  - `PayU`

### Why This Matters
When Cashfree releases *RiskShield* or Razorpay promotes *Chargeback Shield*, WARROOM immediately flags a direct threat to PayFlow’s *FraudShield* product within the *SMB / Mid-Market* segment. If a competitor launches an offline POS terminal, WARROOM classifies it with minimal product overlap because PayFlow does not offer hardware POS systems.

---

## 5. Current MVP: What Is Implemented Today

The current codebase (Phase 3B) provides a fully functioning, end-to-end backend pipeline:

- [x] **Live Web Research Layer**: Synchronous querying of the Anakin Search API (`https://api.anakin.io/v1/search`) for each configured competitor, extracting 3 real results per competitor (titles, URLs, snippets, dates).
- [x] **Context-Aware Prompt Generation**: Constructing targeted search queries dynamically combining competitor names, company products, and target segments.
- [x] **Evidence Assembly**: Preserving raw search outputs with verified URLs and snippets.
- [x] **DronaHQ Reasoning Integration**: Packaging research context into a structured payload dispatched to the DronaHQ webhook URL.
- [x] **Synchronous Output Ingestion**: Consuming the DronaHQ Standard response directly from the `response` field.
- [x] **Resilient Parsing & Normalization**: Stripping markdown fences, parsing JSON, and normalizing diverse signal keys (`competitive_signals`, `change`, `signal_classification`, `significance_explanation`) into a uniform `CompetitiveSignal` schema.
- [x] **Source Evidence Linking**: Mapping collected Anakin URLs back to the normalized signals.
- [x] **Safe Error Handling**: Preserving raw research data if the reasoning agent fails, and refusing to hallucinate signals if the agent reports `insufficient_evidence`.
- [x] **Automated Test Suite**: 11 comprehensive unit tests covering API formatting, malformed responses, error states, and health checks.

---

## 6. Intentionally Excluded Features (Out of Scope for MVP)

To maintain focus, avoid unnecessary complexity, and meet hackathon deadlines, the following capabilities are **intentionally not implemented** in the current release:

1. **Simulated Response Generator / "What If" Sandbox**: Interactive simulation of pricing counter-moves.
2. **"Ask WARROOM" Conversational Chatbot**: Conversational Q&A interface over past signals.
3. **Background Daemon / Cron Monitors**: Automated hourly/daily scraping loops.
4. **External Alerting Channels**: Slack webhooks, email digests, or SMS alerts.
5. **Persistent Database (PostgreSQL / SQLite / Vector DB)**: All scan operations are currently stateless and on-demand.
6. **Multi-Tenant User Authentication**: No user logins, sessions, or API token issuance.
7. **Asynchronous Polling Infrastructure**: No Celery, Redis, or polling loops for external thread IDs.
