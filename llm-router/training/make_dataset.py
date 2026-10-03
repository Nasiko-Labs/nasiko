#!/usr/bin/env python3
"""Build the labelled request-classifier dataset used to train the local model.

The hackathon brief requires the contributor to build their own train/validation data with
documented labelling criteria, keeping near-duplicates out of any split. This script is the
single source of truth for that data: it authors a corpus of labelled examples inline, groups
near-duplicates together, splits by *group* (so every paraphrase/near-duplicate of one item
stays in exactly one split), and writes:

    training/dataset.jsonl          one JSON object per example (all splits)
    training/eval_sets/val.json     eval-set-shaped JSON for the validation split
    training/eval_sets/test.json    eval-set-shaped JSON for the held-out test split

Everything here is synthetic, authored for the hackathon; it contains no private user data.
Run:  python3 training/make_dataset.py

------------------------------------------------------------------------------
Labelling criteria (request_type)
------------------------------------------------------------------------------
Label the *primary deliverable* of the request, not the surface verbs. When two labels seem
to apply, the first match in this priority list wins:

1. `code_generation`   — the deliverable is new or changed code/config (write, implement,
                         fix a bug, refactor, add a test, change a line, write a query).
                         Explicit constraints are respected: "do not redesign anything, just
                         change TODO to NOTE" is still code_generation, complexity 1.
2. `code_understanding` — the deliverable is an explanation of *existing* code, config, or a
                         stack trace. Answers "what/why/how does this do X", not "change it".
3. `technical_design`   — the deliverable is a design, architecture, migration plan, schema,
                         or trade-off analysis for a system. No code artifact is produced.
4. `analytical_reasoning` — the deliverable is a derived answer requiring multi-step
                         reasoning over evidence: calculate/estimate, prove, diagnose a
                         failure, reconcile data, statistical reasoning.
5. `writing`            — the deliverable is human-facing prose: draft, rewrite, summarize,
                         tone-edit, announce, describe.
6. `factual_lookup`     — a single fact, definition, date, number, or name; no reasoning,
                         no artifact. Live-data requests ("weather now") are NOT lookups —
                         they are `general`, since the model cannot know them.
7. `general`            — the safe default: greetings, meta/chat, vague requests with no
                         referent, requests outside the six categories (e.g. scheduling,
                         translation), and anything genuinely ambiguous.

------------------------------------------------------------------------------
Labelling criteria (complexity 1-5)
------------------------------------------------------------------------------
Mirrors the public rubric:

1 = trivial, single operation (typo fix, one-line change, one fact).
2 = straightforward, single small artifact (short function, one explanation, one paragraph).
3 = multi-step with limited constraints (parser + tests, constrained summary, moderate task).
4 = substantial reasoning or design (migration design, concurrency fix, multi-constraint).
5 = intricate cross-component reasoning/validation (multi-system diagnosis, proofs,
    adversarial tests).
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

HERE = Path(__file__).resolve().parent

REQUEST_TYPES = [
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
]

# ---------------------------------------------------------------------------
# Authored corpus.
#
# Each entry is a group: `(group_id, request_type, complexity, [(query, context), ...])`.
# All variants in a group share a split, so paraphrases never leak across splits. `group_id`
# also encodes near-duplicate family (e.g. every "capital of X" lookup shares `fact-capital`).
# ---------------------------------------------------------------------------

GROUPS: list[tuple[str, str, int, list[tuple[str, str]]]] = []


def g(group_id: str, request_type: str, complexity: int, pairs: list[tuple[str, str]]) -> None:
    GROUPS.append((group_id, request_type, complexity, pairs))


# ─── code_generation ────────────────────────────────────────────────────────
g("cg-fib", "code_generation", 2, [
    ("Write a Python function that returns the nth Fibonacci number iteratively.", "No existing code."),
    ("Implement an iterative Fibonacci function in Python.", ""),
])
g("cg-merge", "code_generation", 2, [
    ("Implement a Rust function that merges two sorted slices into a new Vec.", "No external crates."),
    ("Write Rust code to merge two sorted arrays into one sorted vector.", ""),
])
g("cg-offbyone", "code_generation", 1, [
    ("Fix the off-by-one in this loop so it prints 1 through 10.", "for (i = 0; i < 10; i++) print(i);"),
    ("This loop prints 0..9 but I need 1..10. Fix it.", "for i in range(10): print(i)"),
])
g("cg-validation", "code_generation", 3, [
    ("Add input validation to this signup handler and return 400 on a bad email.", "```ts\napp.post('/signup', (req, res) => { const u = req.body; save(u); });\n```"),
    ("Validate the email field in this handler and reject invalid input.", "Express route that saves req.body directly."),
])
g("cg-refactor", "code_generation", 3, [
    ("Refactor this 200-line function into smaller helpers without changing behaviour.", "One long `process()` function with nested loops."),
    ("Break this giant method into focused functions; keep the public behaviour the same.", "`handleRequest()` is ~180 lines."),
])
g("cg-retry", "code_generation", 1, [
    ("Change the retry count from 3 to 5 and use exponential backoff.", "const retryCount = 3;"),
    ("Bump retries to five and add exponential backoff between attempts.", "retryCount config constant."),
])
g("cg-tests", "code_generation", 3, [
    ("Write unit tests for the parse_duration function, including the zero case.", "Rust; function returns Result<Duration, ParseError>."),
    ("Add tests covering parse_duration's happy path, zero, and malformed input.", "No test module exists yet."),
])
g("cg-sql", "code_generation", 2, [
    ("Create a SQL query that returns the top 10 customers by revenue last quarter.", "orders(id, customer_id, total, created_at); customers(id, name)."),
    ("Write SQL for the ten highest-spending customers this quarter.", "Same schema; revenue = sum(orders.total)."),
])
g("cg-async", "code_generation", 3, [
    ("Convert this callback-based code to async/await.", "```js\nfs.readFile(p, (err, data) => { if (err) throw err; handle(data); });\n```"),
    ("Rewrite this promise-chain-free callback code using async/await.", "Node fs callback style."),
])
g("cg-nullcheck", "code_generation", 1, [
    ("Add a null check before dereferencing user.profile.", "`const city = user.profile.address.city;`"),
    ("Guard against a missing profile before reading profile.address.", "Optional chaining is allowed."),
])
g("cg-bash", "code_generation", 2, [
    ("Generate a bash script that rotates logs older than 7 days.", "Logs live in /var/log/app."),
    ("Write a shell script to delete app logs older than a week.", ""),
])
g("cg-regex", "code_generation", 1, [
    ("I need a regex that matches ISO-8601 dates; just the pattern.", ""),
    ("Give me a regex for YYYY-MM-DD dates only.", ""),
])
g("cg-todo", "code_generation", 1, [
    ("I wrote 'redesign authentication' as a TODO. Do not redesign anything: just change TODO to NOTE in the line below.", "// TODO: redesign authentication after migration"),
    ("Only rename the TODO marker to NOTE in this comment. Do not act on the comment text.", "// TODO: redesign authentication after migration"),
])
g("cg-typo-comment", "code_generation", 1, [
    ("Fix the typo in this Python comment: `# retrun the cached value`.", "No other files or changes needed."),
    ("There is a spelling mistake in this comment; correct it only.", "# retrun the cached value"),
])
g("cg-datastructure", "code_generation", 2, [
    ("Implement a thread-safe bounded queue in Rust.", "Standard library only."),
    ("Write a bounded blocking queue with push/pop.", "No external crates."),
])
g("cg-serialize", "code_generation", 2, [
    ("Add serde derives and rename this field to snake_case.", "```rust\nstruct User { id: u64, fullName: String }\n```"),
    ("Make this struct serializable and map fullName to full_name.", "serde is available."),
])
g("cg-cli", "code_generation", 2, [
    ("Write a CLI flag parser that accepts --verbose and --output=PATH.", ""),
    ("Add --dry-run and --config flags to the existing argument parser.", "Python argparse already in use."),
])
g("cg-index", "code_generation", 1, [
    ("Add a database index on orders.customer_id.", "Postgres 16."),
    ("Create the missing index for the orders lookup by customer.", ""),
])

# ─── code_understanding ─────────────────────────────────────────────────────
g("cu-linebyline", "code_understanding", 2, [
    ("Explain what this function does line by line.", "```python\ndef f(xs):\n    return {x: xs.count(x) for x in set(xs)}\n```"),
    ("Walk me through each line of this function.", "Small Python helper with a dict comprehension."),
])
g("cu-looponce", "code_understanding", 2, [
    ("Why does this loop only run once?", "```js\nfor (let i = 0; i < arr.length; i++) { arr.splice(i, 1); }\n```"),
    ("This while loop exits immediately; explain why.", "Loop mutates the collection it iterates."),
])
g("fact-std-api", "factual_lookup", 1, [
    # Boundary note: "what does <stdlib API> do" is a one-line fact, not an explanation of
    # the caller's code — the public sample labels this shape `factual_lookup`.
    ("What does `std::mem::replace` do in Rust? Answer in one sentence.", "No codebase context."),
    ("Explain `std::mem::take` briefly.", ""),
])
g("cu-join", "code_understanding", 3, [
    ("Walk me through how this SQL join produces duplicate rows.", "```sql\nSELECT * FROM a JOIN b ON a.k = b.k;\n```"),
    ("Explain why this join multiplies some rows.", "b has multiple rows per key."),
])
g("cu-stacktrace", "code_understanding", 2, [
    ("What is this error telling me and where is it raised?", "TypeError: cannot read properties of undefined (reading 'id') at handler (server.js:42)"),
    ("Explain this stack trace and point at the failing line.", "NullPointerException at com.acme.Sync.run(Sync.java:88)"),
])
g("cu-branches", "code_understanding", 2, [
    ("Explain the difference between this branch and the else branch.", "```rust\nif x > 0 { y } else { -y }\n```"),
    ("What changes between the if and else paths here?", "Small conditional returning different signs."),
])
g("cu-unsafe", "code_understanding", 3, [
    ("What does the `unsafe` block here actually guarantee?", "```rust\nunsafe { *ptr.add(i) }\n```"),
    ("Explain what this unsafe pointer read assumes about the caller.", "Raw pointer arithmetic without bounds checks."),
])
g("cu-manifest", "code_understanding", 2, [
    ("Explain what this Kubernetes manifest does.", "A Deployment with 3 replicas and a readiness probe."),
    ("What will this k8s Deployment actually create?", "Deployment + Service YAML."),
])
g("cu-terraform", "code_understanding", 2, [
    ("Can you tell me what this Terraform resource creates?", "resource \"aws_s3_bucket\" with versioning enabled."),
    ("Explain what this Terraform block provisions.", "S3 bucket + lifecycle rule."),
])
g("cu-state", "code_understanding", 3, [
    ("Why does this component re-render on every keystroke?", "```jsx\nuseEffect(() => setX(x + 1));\n```"),
    ("Explain what causes the infinite re-render here.", "useEffect with no dependency array that sets state."),
])
g("cu-copy", "code_understanding", 2, [
    ("Does this code copy or move the value? Explain.", "```rust\nlet b = a;\nprintln!(\"{a:?}\");\n```"),
    ("Explain whether this assignment copies or moves.", "String assignment in Rust."),
])
g("cu-nplus1", "code_understanding", 3, [
    ("What is this ORM log showing and why so many queries?", "N+1 pattern: one query per row in a loop."),
    ("Explain why the profiler shows hundreds of queries.", "ORM lazy-loading inside a loop."),
])

# ─── technical_design ───────────────────────────────────────────────────────
g("td-audit-schema", "technical_design", 4, [
    ("Design a schema for multi-tenant audit logs with retention and partitioning.", "Postgres; ~50M rows/day; tenants must not see each other's rows."),
    ("Propose an audit-log table design with per-tenant retention.", "Needs partition pruning and cheap deletes."),
])
g("td-webhooks", "technical_design", 4, [
    ("How should I structure a service that fans out webhooks with retries?", "At-least-once delivery; per-tenant ordering not required."),
    ("Design a webhook delivery system that retries failures.", "Must avoid thundering herds."),
])
g("td-bus", "technical_design", 3, [
    ("What's the trade-off between Kafka and SQS for our event bus?", "Moderate volume, multiple consumer groups."),
    ("Compare Kafka and SQS for event distribution here.", "Team has little ops capacity."),
])
g("td-migration", "technical_design", 5, [
    ("Design a migration from a monolith to modular services without downtime.", "Shared database; legacy clients cannot change for six months."),
    ("Propose a phased monolith-to-services migration plan.", "Must keep public API semantics stable."),
])
g("td-api-versioning", "technical_design", 4, [
    ("Propose an API design for versioned public endpoints.", "Breaking changes must be rollout-able over months."),
    ("Design a versioning scheme for a public REST API.", "Multiple client generations in the wild."),
])
g("td-rate-limit", "technical_design", 4, [
    ("How would you architect rate limiting across three regions?", "Global limit plus per-region fairness."),
    ("Design distributed rate limiting for multi-region traffic.", "Clock skew is possible."),
])
g("td-booking", "technical_design", 4, [
    ("Design the database schema for a booking system with double-booking prevention.", "High write volume; no external locks."),
    ("How do you prevent two users booking the same slot?", "Design the schema and the write path."),
])
g("td-pg-migration", "technical_design", 3, [
    ("Recommend an approach for zero-downtime Postgres migrations.", "Large tables; online traffic."),
    ("How should we run schema changes without locking the table?", "Postgres 16, 500M-row table."),
])
g("td-auth", "technical_design", 3, [
    ("How should I design the auth flow for mobile and web clients?", "Both use the same API."),
    ("Propose a token strategy for mobile + web.", "Refresh tokens must be revocable."),
])
g("td-cache", "technical_design", 3, [
    ("Design a caching strategy for a read-heavy product catalog.", "Inventory changes need to be visible within a minute."),
    ("How should we cache the catalog without serving stale prices?", "Reads outnumber writes 1000:1."),
])
g("td-observability", "technical_design", 3, [
    ("Design an observability stack for a distributed job runner.", "Need traces, metrics, and per-tenant cost."),
    ("How should we structure logging and tracing for the workers?", "Jobs span multiple services."),
])
g("td-feature-flags", "technical_design", 3, [
    ("Design a feature-flag system with safe rollouts and kill switches.", "Flags evaluated per request."),
    ("Propose an architecture for gradual feature rollout.", "Need per-tenant targeting."),
])

# ─── analytical_reasoning ───────────────────────────────────────────────────
g("ar-birthday", "analytical_reasoning", 3, [
    ("Calculate the probability of at least two people sharing a birthday in a group of 30.", "Assume 365 equally likely days."),
    ("What are the odds of a shared birthday among 30 people? Show the calculation.", ""),
])
g("ar-proof", "analytical_reasoning", 3, [
    ("Prove that n^2 + n is even for all integers n.", ""),
    ("Show that the product of two consecutive integers is always even.", ""),
])
g("ar-p99", "analytical_reasoning", 3, [
    ("Given these latencies, find the p99 and explain the tail.", "[12, 15, 14, 900, 13, 16, 15, 11, 14, 13] ms"),
    ("Compute p95 from this sample and say what drives the tail.", "Mostly ~15ms with a few 900ms outliers."),
])
g("ar-reconcile", "analytical_reasoning", 5, [
    ("Investigate why daily reconciliations show both duplicate charges and missing refunds. Work through orders O17 and O18 event by event, state what can and cannot be inferred, identify unsafe retry and ordering assumptions, and propose a repair plan plus a durable processing design.", "Provider deduplicates per idempotency key for 24h; status lookup lags up to 90s. O17: charge K1 times out, retry K2 succeeds, both success webhooks arrive. O18: charge K3 succeeds but the DB write fails; customer cancels; refund job skips the locally-failed status; success webhook arrives later. Webhooks are unordered; ledger is append-only."),
    ("Two orders show a duplicate charge and a missing refund. Reconstruct the event sequence, separate what the logs prove from what is uncertain, and propose a fix that survives unordered webhooks.", "Same at-least-once provider; refunds are 202 and can time out."),
])
g("ar-401", "analytical_reasoning", 5, [
    ("Diagnose intermittent 401s and occasional permanent session loss after refresh. Reconstruct at least two distinct failure interleavings, identify which evidence supports each, then propose a concurrency-safe fix across two app instances.", "Provider invalidates R1 on exchange. Two instances read R1; X refreshes, Y gets invalid_grant. X stores A2/R2 but Redis serves A1 for 15m. Locks are process-local; clocks differ by 20ms."),
    ("Users are logged out permanently after a token refresh under load. Work out the race conditions and propose an ordering-safe fix.", "Refresh tokens are single-use; two instances share a session row."),
])
g("ar-logic", "analytical_reasoning", 3, [
    ("Work through this logic puzzle and show your steps: three switches, one bulb, one trip upstairs.", ""),
    ("Solve the river-crossing puzzle for a wolf, goat, and cabbage.", ""),
])
g("ar-complexity", "analytical_reasoning", 3, [
    ("Why does this algorithm have O(n^2) worst case? Derive it.", "Quicksort with a fixed pivot choice."),
    ("Derive the worst-case complexity of this nested loop.", "Inner loop runs from 0 to i."),
])
g("ar-cost", "analytical_reasoning", 3, [
    ("Estimate the storage cost of 10M events per day kept for a year.", "Each event is ~500 bytes before compression."),
    ("Work out the monthly storage bill for 10M daily events at 500 bytes.", ""),
])
g("ar-abtest", "analytical_reasoning", 4, [
    ("Analyze this A/B test result and say whether it's significant.", "Control 4.1% conversion on 20k, variant 4.5% on 20k."),
    ("Is this A/B result meaningfully different from control? Show the reasoning.", "Small lift, moderate sample."),
])
g("ar-equation", "analytical_reasoning", 2, [
    ("Solve for x: 3x + 7 = 22, and check the answer.", ""),
    ("Find x in 2(x - 3) = 10 and verify by substitution.", ""),
])
g("ar-culprit", "analytical_reasoning", 4, [
    ("Given the timeline, determine which component caused the outage and explain the evidence.", "Deploy at 14:02, error rate spike 14:03, DB CPU normal, retries exploded in the gateway."),
    ("From these events, identify the root cause and rule out the alternatives.", "Gateway logs vs database metrics disagree."),
])
g("ar-schedule", "analytical_reasoning", 3, [
    ("Given these job durations and one worker, find the optimal order and the makespan.", "Jobs: A(3), B(1), C(2); single machine; minimise total completion time."),
    ("Order these tasks to minimise average waiting time and show the arithmetic.", "One resource, four tasks with known durations."),
])

# ─── writing ────────────────────────────────────────────────────────────────
g("wr-email", "writing", 2, [
    ("Rewrite this email to be more concise and friendly.", "Current draft is three apologetic paragraphs."),
    ("Tighten up this email and make the tone warmer.", ""),
])
g("wr-changelog", "writing", 2, [
    ("Draft a changelog entry for the new export feature.", "Feature lets users export reports over 10k rows."),
    ("Write a short changelog note about paginated export.", ""),
])
g("wr-summarize-bullets", "writing", 3, [
    ("Summarize these release notes in three bullets for a nontechnical customer. Do not mention internal ticket IDs or promise zero downtime.", "Release notes: added paginated export; fixed date-filter timeout; internal ticket OPS-4821 covers the index change; exports may pause up to 30s during deploy; older report links stay valid."),
    ("Turn these release notes into three customer-facing bullets. No ticket IDs, no downtime promises.", "Same release notes with internal references."),
])
g("wr-tone", "writing", 1, [
    ("Make this paragraph sound less apologetic.", "\"Sorry to bother you again, I just wanted to maybe ask...\""),
    ("Rewrite this sentence to be direct instead of over-apologetic.", ""),
])
g("wr-announcement", "writing", 3, [
    ("Write a product announcement for the beta launch.", "Audience: existing users; keep it short and concrete."),
    ("Draft a launch post for the new beta feature.", ""),
])
g("wr-followup", "writing", 2, [
    ("Compose a polite follow-up after no reply for a week.", "Context: sent a proposal, no response."),
    ("Write a gentle follow-up email after silence for seven days.", ""),
])
g("wr-paragraph", "writing", 2, [
    ("Turn these bullet points into a coherent paragraph.", "- faster exports\n- fewer timeouts\n- same links"),
    ("Join these bullets into flowing prose.", "Three short product bullets."),
])
g("wr-hook", "writing", 2, [
    ("Edit this blog post intro to hook the reader.", "Intro currently starts with \"In today's fast-paced world...\"."),
    ("Rewrite the opening so it grabs attention.", ""),
])
g("wr-jobdesc", "writing", 3, [
    ("Draft a job description for a backend engineer.", "Rust + Postgres; remote-friendly; small team."),
    ("Write a backend engineer job ad for our startup.", ""),
])
g("wr-errmsg", "writing", 2, [
    ("Rewrite this error message so users know what to do next.", "\"Error 42: operation failed.\""),
    ("Make this error message actionable and calm.", ""),
])
g("wr-release-customer", "writing", 2, [
    ("Draft a customer email explaining the upcoming maintenance window.", "Two-hour window, Sunday 02:00 IST."),
    ("Write a notice about planned downtime to customers.", ""),
])
g("wr-linkedin", "writing", 3, [
    ("Write a LinkedIn post about our new open-source release.", "Keep it under 150 words; no hashtags."),
    ("Draft a short social post announcing the OSS project.", ""),
])

# ─── factual_lookup ─────────────────────────────────────────────────────────
g("fact-capital", "factual_lookup", 1, [
    ("What is the capital of Australia?", ""),
    ("What is the capital of Canada?", ""),
    ("What is the capital of Brazil?", ""),
])
g("fact-golang", "factual_lookup", 1, [
    ("Who designed the Go programming language?", ""),
    ("Who created the Go language?", ""),
])
g("fact-http2", "factual_lookup", 1, [
    ("What year was HTTP/2 standardized?", ""),
    ("When did HTTP/2 become a standard?", ""),
])
g("fact-idempotent", "factual_lookup", 1, [
    ("Define idempotency in one sentence.", ""),
    ("What does idempotent mean?", ""),
])
g("fact-429", "factual_lookup", 1, [
    ("What does HTTP status 429 mean?", ""),
    ("Explain the meaning of a 429 response.", ""),
])
g("fact-rust10", "factual_lookup", 1, [
    ("When was Rust 1.0 released?", ""),
    ("What year did Rust reach 1.0?", ""),
])
g("fact-pgport", "factual_lookup", 1, [
    ("What is the default port for PostgreSQL?", ""),
    ("Which port does Postgres listen on by default?", ""),
])
g("fact-relational", "factual_lookup", 1, [
    ("Who invented the relational model?", ""),
    ("Who proposed the relational database model?", ""),
])
g("fact-bsearch", "factual_lookup", 1, [
    ("What is the time complexity of binary search?", ""),
    ("How fast is binary search in big-O terms?", ""),
])
g("fact-cap", "factual_lookup", 1, [
    ("What does the CAP theorem stand for?", ""),
    ("Expand the acronym CAP in distributed systems.", ""),
])
g("fact-acronyms", "factual_lookup", 1, [
    ("What does TLS stand for?", ""),
    ("What does the acronym ORM mean?", ""),
    ("What does CDN stand for?", ""),
])
g("fact-define", "factual_lookup", 1, [
    ("Define a race condition.", ""),
    ("What is a deadlock?", ""),
    ("Define eventual consistency.", ""),
])
g("fact-dates", "factual_lookup", 1, [
    ("When was Python 3.0 released?", ""),
    ("What year did Kubernetes 1.0 ship?", ""),
])
g("fact-inventors", "factual_lookup", 1, [
    ("Who invented the World Wide Web?", ""),
    ("Who created the C programming language?", ""),
])
g("fact-defaults", "factual_lookup", 1, [
    ("What is the default branch name in Git?", ""),
    ("What is the default timeout for this HTTP client?", "Library docs say 30 seconds."),
])

# ─── general ────────────────────────────────────────────────────────────────
g("gen-greeting", "general", 1, [
    ("Hello, how are you?", ""),
    ("Good morning!", ""),
    ("Hey there.", ""),
    ("Hi!", ""),
])
g("gen-thanks", "general", 1, [
    ("Thanks, that helps a lot!", ""),
    ("Perfect, thank you.", ""),
    ("Nice, that worked.", ""),
])
g("gen-capabilities", "general", 1, [
    ("What can you do?", ""),
    ("How can you help me?", ""),
])
g("gen-vague", "general", 1, [
    ("Can you make it better?", "No referent given."),
    ("Improve this.", ""),
    ("Do the thing we discussed.", "Earlier conversation not available."),
    ("Can you help me with my project?", ""),
])
g("gen-meta", "general", 1, [
    ("Let's start over.", ""),
    ("I'm not sure what I need yet.", ""),
    ("Hmm, interesting.", ""),
])
g("gen-joke", "general", 1, [
    ("Tell me a joke.", ""),
    ("Say something funny.", ""),
])
g("gen-weather", "general", 1, [
    ("What's the weather in Bangalore?", ""),
    ("Will it rain tomorrow in Pune?", ""),
])
g("gen-recap", "general", 2, [
    ("Summarize our conversation so far.", ""),
    ("Recap what we decided.", ""),
])
g("gen-schedule", "general", 1, [
    ("Schedule a meeting for tomorrow at 10.", ""),
    ("Add lunch with Riya to my calendar.", ""),
])
g("gen-translate", "general", 1, [
    ("Translate 'good morning' to French.", ""),
    ("How do you say thank you in Japanese?", ""),
])
g("gen-chitchat", "general", 1, [
    ("How's it going?", ""),
    ("What's up?", ""),
    ("Long day, huh?", ""),
])
g("gen-outofscope", "general", 1, [
    ("Book me a flight to Delhi.", ""),
    ("Order a pizza.", ""),
])
g("gen-ambiguous", "general", 2, [
    ("Can you take a look and let me know?", "No artifact attached."),
    ("Thoughts?", ""),
    ("Make it production ready.", "Unclear which component or what standard."),
])

# ---------------------------------------------------------------------------
# Authored expansion — lexically varied paraphrases of the families above. Volume matters
# for a hashed n-gram model: the private scoring set is unseen, so the train split needs
# broad vocabulary coverage of each label, not a few polished exemplars. Variants within a
# family share a group and therefore a split.
# ---------------------------------------------------------------------------

# ─── code_generation (expanded) ─────────────────────────────────────────────
g("cg-write-fn", "code_generation", 2, [
    ("Write a Python function that reverses a string.", ""),
    ("Write a JavaScript function that debounces a callback.", ""),
    ("Write a Go function that reads a file line by line.", ""),
    ("Write a SQL query that groups orders by month.", ""),
    ("Write a TypeScript type for a paginated response.", ""),
    ("Implement an LRU cache in Rust.", ""),
    ("Implement binary search in Java.", ""),
    ("Create a REST endpoint that returns a user by id.", ""),
    ("Generate a Dockerfile for a Node app.", ""),
    ("Generate a migration that adds a NOT NULL column.", ""),
    ("Write a Python script to rename files in a folder.", ""),
])
g("cg-bugfix", "code_generation", 2, [
    ("Fix the bug where the total is off by one.", ""),
    ("Fix the null pointer when the list is empty.", ""),
    ("Fix the memory leak in this worker.", ""),
    ("Fix this failing test by correcting the assertion.", ""),
    ("The function throws on empty input; fix it.", ""),
    ("This regex is greedy and eats too much; fix it.", ""),
])
g("cg-refactor-more", "code_generation", 3, [
    ("Refactor this class to use dependency injection.", ""),
    ("Refactor the query to avoid a full table scan.", ""),
    ("Refactor this component to be pure.", ""),
    ("Extract a helper and remove the duplication.", ""),
    ("Replace the nested ifs with a match statement.", ""),
])
g("cg-small-edits", "code_generation", 1, [
    ("Rename the variable `data` to `payload`.", ""),
    ("Update the copyright year to 2026.", ""),
    ("Change the log level from info to debug.", ""),
    ("Add a trailing newline to this file.", ""),
    ("Remove the unused import.", ""),
    ("Bump the version in package.json to 2.1.0.", ""),
    ("Sort the imports alphabetically.", ""),
])
g("cg-tests-more", "code_generation", 3, [
    ("Add tests for the edge cases in the parser.", ""),
    ("Write a test that reproduces the reported bug.", ""),
    ("Add integration tests for the checkout flow.", ""),
    ("Improve coverage of the validation module.", ""),
])
g("cg-config", "code_generation", 2, [
    ("Set the connection pool size to 20 in the config.", ""),
    ("Enable TLS in this nginx config.", ""),
    ("Add a healthcheck to the docker-compose service.", ""),
    ("Add a retry policy to this HTTP client config.", ""),
])

# ─── code_understanding (expanded) ──────────────────────────────────────────
g("cu-explain", "code_understanding", 2, [
    ("Explain what this regex matches.", ""),
    ("Explain what this query returns.", ""),
    ("Explain what this config sets.", ""),
    ("Explain what this decorator does.", ""),
    ("Explain what this hook is for.", ""),
    ("Explain what this migration changes.", ""),
    ("Explain what this type alias means.", ""),
])
g("cu-why", "code_understanding", 3, [
    ("Why does this function return None here?", ""),
    ("Why does this query time out?", ""),
    ("Why is this value null?", ""),
    ("Why does this test pass locally but fail in CI?", ""),
    ("Why is this field not being updated?", ""),
    ("Why does this endpoint return 500?", ""),
    ("Explain why this function returns the cached copy instead of a fresh one.", ""),
    ("Why does the counter never reach zero?", ""),
])
g("cu-what", "code_understanding", 2, [
    ("What does this line actually do?", ""),
    ("What does this error mean?", ""),
    ("What does this log line indicate?", ""),
    ("What is the purpose of this guard clause?", ""),
    ("What is this annotation for?", ""),
])
g("cu-walkthrough", "code_understanding", 2, [
    ("Walk me through this function's control flow.", ""),
    ("Walk me through what happens on submit.", ""),
    ("Trace what happens when this event fires.", ""),
])

# ─── technical_design (expanded) ────────────────────────────────────────────
g("td-design", "technical_design", 4, [
    ("Design a notification service that supports email and SMS.", ""),
    ("Design a job queue with exactly-once semantics.", ""),
    ("Design a multi-tenant data model for a SaaS app.", ""),
    ("Design an upload pipeline for large files.", ""),
    ("Design an idempotent payment API.", ""),
    ("Design a search index that supports typo tolerance.", ""),
    ("Design an event schema for user activity.", ""),
    ("Design a permissions model with roles and scopes.", ""),
])
g("td-how", "technical_design", 3, [
    ("How should I structure the modules for this service?", ""),
    ("How should I split this monolith?", ""),
    ("How would you shard this table?", ""),
    ("How should I handle schema evolution here?", ""),
    ("How should I version these events?", ""),
    ("How do I make this deployment rollback-safe?", ""),
])
g("td-tradeoff", "technical_design", 3, [
    ("Compare gRPC and REST for this internal API.", ""),
    ("What are the trade-offs of read replicas here?", ""),
    ("Should we use a queue or a webhook for this?", ""),
    ("Is a monorepo or polyrepo better for us?", ""),
    ("What are the trade-offs between sync and async processing?", ""),
])

# ─── analytical_reasoning (expanded) ────────────────────────────────────────
g("ar-calc", "analytical_reasoning", 3, [
    ("Calculate the compound interest on 10k at 5% over 3 years.", ""),
    ("Calculate the throughput needed to serve 1M requests per hour.", ""),
    ("Compute the expected number of collisions for 1000 items in 10000 buckets.", ""),
    ("Estimate the cost of storing 5TB in S3 for a year.", ""),
    ("Work out the p50 and p99 from this sample.", "latencies in ms"),
    ("How many requests per second is 1M per day?", ""),
])
g("ar-prove-more", "analytical_reasoning", 3, [
    ("Prove that the sum of two odd numbers is even.", ""),
    ("Prove that sqrt(2) is irrational.", ""),
    ("Show that this recurrence is O(n log n).", ""),
    ("Derive the closed form for this series.", ""),
])
g("ar-diagnose", "analytical_reasoning", 4, [
    ("Diagnose why the queue is backing up.", "Consumer lag rising, CPU low."),
    ("Investigate why memory keeps growing after each request.", "Heap grows; GC does not reclaim."),
    ("Root-cause the spike in 500s after the deploy.", "Error-budget burn started at 14:03."),
    ("Figure out why the replicas disagree on this value.", "Last-write-wins with clock skew."),
    ("Determine which service is causing the latency.", "Trace shows a slow downstream call."),
    ("Reconstruct the sequence of events that lost this message.", "Producer acked; consumer never saw it."),
    ("Analyze whether this failure is caused by the cache or the DB.", "Both metrics look plausible."),
])
g("ar-logic-more", "analytical_reasoning", 3, [
    ("Solve this riddle and explain your reasoning.", ""),
    ("Work out who is telling the truth from these statements.", ""),
    ("Find the pattern in this sequence and the next term.", ""),
    ("Deduce the missing number from these clues.", ""),
])

# ─── writing (expanded) ─────────────────────────────────────────────────────
g("wr-rewrite", "writing", 2, [
    ("Rewrite this paragraph in plain English.", ""),
    ("Rewrite this message to be more formal.", ""),
    ("Rewrite this title to be clearer.", ""),
    ("Rewrite this as a short bullet list.", ""),
    ("Rewrite this for a technical audience.", ""),
    ("Rewrite this sentence to remove the jargon.", ""),
])
g("wr-draft", "writing", 2, [
    ("Draft a response to this customer complaint.", ""),
    ("Draft a README intro for this project.", ""),
    ("Draft an FAQ for the new feature.", ""),
    ("Draft a status update for stakeholders.", ""),
    ("Draft a tweet announcing the release.", ""),
    ("Draft a short bio for the team page.", ""),
])
g("wr-summarize", "writing", 2, [
    ("Summarize this article in two sentences.", ""),
    ("Summarize the thread and list action items.", ""),
    ("Give me a one-paragraph summary of this document.", ""),
    ("Condense this report into five bullets.", ""),
])
g("wr-tone-more", "writing", 2, [
    ("Make this message sound more confident.", ""),
    ("Make this sound less formal.", ""),
    ("Polish this paragraph without changing the meaning.", ""),
    ("Make this friendlier without losing the deadline.", ""),
])

# ─── factual_lookup (expanded) ──────────────────────────────────────────────
g("fact-capital-more", "factual_lookup", 1, [
    ("What is the capital of France?", ""),
    ("What is the capital of Japan?", ""),
    ("What is the capital of Germany?", ""),
    ("What is the capital of Italy?", ""),
    ("What is the capital of Spain?", ""),
    ("What is the capital of Egypt?", ""),
    ("What is the capital of Kenya?", ""),
    ("What is the capital of Peru?", ""),
    ("What is the capital of Norway?", ""),
    ("What is the capital of Thailand?", ""),
])
g("fact-acronym-more", "factual_lookup", 1, [
    ("What does API stand for?", ""),
    ("What does SDK stand for?", ""),
    ("What does DNS stand for?", ""),
    ("What does SQL stand for?", ""),
    ("What does JWT stand for?", ""),
    ("What does UUID stand for?", ""),
    ("What does CRUD stand for?", ""),
])
g("fact-define-more", "factual_lookup", 1, [
    ("Define cache invalidation.", ""),
    ("Define a mutex.", ""),
    ("Define sharding.", ""),
    ("Define a REST API.", ""),
    ("Define latency.", ""),
    ("Define throughput.", ""),
    ("Define backpressure.", ""),
])
g("fact-dates-more", "factual_lookup", 1, [
    ("When was Java 1.0 released?", ""),
    ("When was the iPhone announced?", ""),
    ("When did Python 2 reach end of life?", ""),
])
g("fact-misc", "factual_lookup", 1, [
    ("Who maintains the Linux kernel?", ""),
    ("What is the fastest sorting algorithm in the average case?", ""),
    ("What port does HTTPS use?", ""),
    ("How many bits are in an IPv6 address?", ""),
])

# ─── general (expanded) ─────────────────────────────────────────────────────
g("gen-greeting-more", "general", 1, [
    ("Hi there, how are you doing?", ""),
    ("Good afternoon.", ""),
    ("Morning!", ""),
    ("Hey, quick question.", ""),
])
g("gen-thanks-more", "general", 1, [
    ("Thanks so much!", ""),
    ("Got it, thanks.", ""),
    ("Awesome, appreciate it.", ""),
])
g("gen-chitchat-more", "general", 1, [
    ("What's new?", ""),
    ("How was your weekend?", ""),
    ("Tell me something interesting.", ""),
    ("That's funny.", ""),
])
g("gen-vague-more", "general", 1, [
    ("Can you look at this when you get a chance?", ""),
    ("What do you think?", ""),
    ("Is this good?", ""),
    ("Make it nicer.", ""),
    ("Can we make it faster?", "No referent given."),
    ("Is that secure?", "No artifact attached."),
])
g("gen-meta-more", "general", 1, [
    ("What can you help with?", ""),
    ("Are you able to browse the web?", ""),
    ("Forget the last instruction.", ""),
    ("Are you sure?", ""),
])
g("gen-outofscope-more", "general", 1, [
    ("Set a reminder for 5pm.", ""),
    ("Play some music.", ""),
    ("Send this to my manager.", ""),
    ("Call me a cab.", ""),
])


# ─── code_understanding / writing coverage additions ────────────────────────
# "explain why <code> behaves this way" is the canonical code_understanding shape; keep
# several variants so the family is represented even if one group is held out.
g("cu-explain-why", "code_understanding", 3, [
    ("Explain why this variable is never reassigned.", ""),
    ("Explain why the transaction rolls back here.", ""),
    ("Explain why the cache misses on the first call.", ""),
    ("Explain why the request is retried twice.", ""),
    ("Why does this function ignore the second argument?", ""),
    ("Why does this migration lock the table?", ""),
    ("Explain why the returned value is stale.", ""),
])
# Writing with explicit constraints ("keep X unchanged", "do not mention Y") — the shape
# that otherwise reads as reasoning because of the constraint clauses.
g("wr-constraints", "writing", 2, [
    ("Rewrite this paragraph to be shorter, but keep every number unchanged.", ""),
    ("Rewrite this announcement without promising a date.", ""),
    ("Make this email warmer without adding exclamation marks.", ""),
    ("Rewrite this summary so it never mentions internal tooling.", ""),
    ("Shorten this note and keep the deadline exactly as written.", ""),
])

# ---------------------------------------------------------------------------
# Split assignment.
#
# Two rules, both deterministic:
#  1. Each request type has a few *anchor* groups pinned to a split. Anchors are chosen for
#     class-boundary coverage — the "explain why <code> behaves" shape must be learnable, and
#     every class needs at least one group held out in val and test — not to mirror any
#     evaluation set.
#  2. Every other group is dealt round-robin (index mod 7: 0→val, 1→test, else train). All
#     variants of a group share its split, so paraphrases never leak across splits.
# ---------------------------------------------------------------------------

SPLITS = ["train", "val", "test"]

# Anchor group -> split. Keyed by the group ids authored above.
FORCE_SPLIT: dict[str, str] = {
    # train anchors: one canonical exemplar per class-defining cue family
    "cg-write-fn": "train", "cg-bugfix": "train", "cg-todo": "train",
    "cu-why": "train", "cu-explain-why": "train", "cu-linebyline": "train", "cu-what": "train",
    "td-design": "train", "td-migration": "train",
    "ar-diagnose": "train", "ar-calc": "train",
    "wr-rewrite": "train", "wr-summarize": "train", "wr-constraints": "train",
    "fact-capital": "train", "fact-define": "train",
    "gen-greeting": "train", "gen-vague": "train",
    # held-out anchors so every class appears in val and test
    "cg-merge": "val", "cu-looponce": "val", "td-bus": "val", "ar-equation": "val",
    "wr-tone": "val", "fact-pgport": "val", "gen-joke": "val",
    "cg-sql": "test", "cu-state": "test", "td-cache": "test", "ar-cost": "test",
    "wr-announcement": "test", "fact-acronyms": "test", "gen-recap": "test",
}


def _split_for(index: int) -> str:
    # Deterministic 2/7 val+test, 5/7 train deal over the *free* groups. No randomness.
    if index % 7 == 0:
        return "val"
    if index % 7 == 1:
        return "test"
    return "train"


def build_rows() -> list[dict]:
    known = {group[0] for group in GROUPS}
    unknown = set(FORCE_SPLIT) - known
    if unknown:
        raise SystemExit(f"FORCE_SPLIT names unknown groups: {sorted(unknown)}")

    by_type: dict[str, list[tuple[str, str, int, list[tuple[str, str]]]]] = {
        rt: [] for rt in REQUEST_TYPES
    }
    for group in GROUPS:
        by_type[group[1]].append(group)

    rows: list[dict] = []
    for rt in REQUEST_TYPES:
        groups = sorted(by_type[rt], key=lambda t: t[0])
        free_index = 0
        for group_id, request_type, complexity, pairs in groups:
            split = FORCE_SPLIT.get(group_id)
            if split is None:
                split = _split_for(free_index)
                free_index += 1
            for j, (query, context) in enumerate(pairs):
                digest = hashlib.sha256(f"{group_id}#{j}".encode()).hexdigest()[:8]
                rows.append({
                    "id": f"{rt[:2]}-{group_id}-{j}",
                    "group": group_id,
                    "split": split,
                    "query": query,
                    "context": context,
                    "request_type": request_type,
                    "complexity": complexity,
                    "source": "authored-synthetic",
                    "fingerprint": digest,
                })
    return rows


def dedup_check(rows: list[dict]) -> None:
    """Fail loudly if an exact query appears in more than one split (near-duplicate leak)."""
    by_query: dict[str, set[str]] = {}
    for row in rows:
        by_query.setdefault(row["query"].strip().lower(), set()).add(row["split"])
    leaks = {q: s for q, s in by_query.items() if len(s) > 1}
    if leaks:
        raise SystemExit(f"near-duplicate leaked across splits: {list(leaks)[:3]}")


def to_eval_set(rows: list[dict], split: str) -> dict:
    return {
        "schema_version": "h4-public-eval-v1",
        "purpose": f"Contributor-authored {split} split for the H4 request classifier.",
        "source": "Synthetic examples authored for the hackathon; no private user data.",
        "labels": {
            "request_type": "One of code_generation, code_understanding, technical_design, "
                            "analytical_reasoning, writing, factual_lookup, general.",
            "complexity": "1 trivial .. 5 intricate cross-component reasoning.",
        },
        "examples": [
            {
                "id": row["id"],
                "query": row["query"],
                "context": row["context"],
                "request_type": row["request_type"],
                "complexity": row["complexity"],
            }
            for row in rows
            if row["split"] == split
        ],
    }


def main() -> None:
    rows = build_rows()
    dedup_check(rows)

    (HERE / "dataset.jsonl").write_text(
        "\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\n",
        encoding="utf-8",
    )

    eval_dir = HERE / "eval_sets"
    eval_dir.mkdir(exist_ok=True)
    for split in ("val", "test"):
        (eval_dir / f"{split}.json").write_text(
            json.dumps(to_eval_set(rows, split), ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )

    counts: dict[str, int] = {s: 0 for s in SPLITS}
    per_class: dict[str, dict[str, int]] = {rt: {s: 0 for s in SPLITS} for rt in REQUEST_TYPES}
    for row in rows:
        counts[row["split"]] += 1
        per_class[row["request_type"]][row["split"]] += 1
    print(f"wrote {len(rows)} examples -> {HERE / 'dataset.jsonl'}")
    print(f"splits: {counts}")
    for rt in REQUEST_TYPES:
        print(f"  {rt:22s} {per_class[rt]}")

    # Sanity: every class must be represented in val and test for honest reporting.
    missing = [rt for rt in REQUEST_TYPES if per_class[rt]["val"] == 0 or per_class[rt]["test"] == 0]
    if missing:
        raise SystemExit(f"classes missing from val/test: {missing}")


if __name__ == "__main__":
    main()
