"""Build the labelled request-classifier dataset (see DATA.md for the labelling rules).

Hand-written seeds -> deterministic augmentation -> train/val split grouped by seed, so a seed and
all of its paraphrase/noise variants always land in the same split (no near-duplicate leakage).

    python build_dataset.py   # writes data/train.jsonl and data/val.jsonl
"""

import json
import random
from pathlib import Path

# (query, context, request_type, complexity 1-5)
SEEDS = [
    # ---------------- code_generation: the deliverable is new or changed code ----------------
    ("Rename the variable `tmp` to `buffer` in this function.", "", "code_generation", 1),
    ("Write a function that reverses a string in JavaScript.", "", "code_generation", 1),
    ("Add a --verbose flag to my argparse CLI that sets logging to DEBUG.", "Python 3.11 script, argparse already used.", "code_generation", 2),
    ("Write a SQL query returning the top 5 customers by total order value in the last 30 days.", "Tables: customers(id,name), orders(id,customer_id,total,created_at).", "code_generation", 2),
    ("Create a React component for a paginated table with sortable columns.", "TypeScript, no UI library.", "code_generation", 3),
    ("Generate a Dockerfile for a FastAPI app using uvicorn, multi-stage, non-root user.", "", "code_generation", 2),
    ("Build a retry decorator with exponential backoff and jitter, configurable on exception types.", "Python 3.12.", "code_generation", 3),
    ("Write unit tests for this function covering empty input and unicode.", "def slugify(s: str) -> str: ...", "code_generation", 2),
    ("Convert this callback-based Node function to async/await.", "fs.readFile with nested callbacks, 12 lines.", "code_generation", 2),
    ("Implement an LRU cache with O(1) get and put and thread safety in Go.", "", "code_generation", 3),
    ("Write a bash script that backs up a Postgres database to S3 daily and prunes backups older than 14 days.", "", "code_generation", 3),
    ("Add input validation to this endpoint so negative quantities return a 422.", "Axum handler, serde struct Order { qty: i32 }.", "code_generation", 2),
    ("Implement a rate limiter middleware using a token bucket, backed by Redis, with per-API-key limits.", "Rust, tower middleware, existing redis client.", "code_generation", 4),
    ("Write a regex that matches ISO 8601 dates with optional time and timezone.", "", "code_generation", 2),
    ("Port this Python script to Rust, keeping the CLI interface identical.", "200-line script using requests and csv.", "code_generation", 4),
    ("Write a GitHub Actions workflow that runs cargo test and clippy on every pull request.", "", "code_generation", 2),
    ("Implement a diff algorithm for two JSON documents that outputs RFC 6902 patches.", "TypeScript, no dependencies.", "code_generation", 4),
    ("Make this loop use iterators instead of indexing.", "for i in 0..v.len() { sum += v[i]; }", "code_generation", 1),
    ("write me a python snippet to read a csv into a dict", "", "code_generation", 1),
    ("Refactor this 300-line handler into smaller functions without changing behaviour, and add tests.", "Go HTTP handler for order checkout.", "code_generation", 4),
    ("Add a Kubernetes liveness and readiness probe to this deployment yaml.", "Service listens on 8080, /healthz exists.", "code_generation", 2),
    # ---------------- code_understanding: explain/trace/debug existing code ----------------
    ("What does this regex do? `^(?=.*[A-Z])(?=.*\\d).{8,}$`", "", "code_understanding", 2),
    ("Walk me through what this SQL query computes.", "SELECT d, SUM(x) OVER (PARTITION BY d ORDER BY t ROWS BETWEEN 6 PRECEDING AND CURRENT ROW) FROM m;", "code_understanding", 2),
    ("Why does this React component re-render on every keystroke?", "useState in the parent holds the whole form object; children are not memoized.", "code_understanding", 3),
    ("What is the difference between `&str` and `String` in this snippet and why does it fail to compile?", "let s: &str = String::from(\"a\");", "code_understanding", 2),
    ("Summarize what this module is responsible for.", "src/routing/boundary.rs, 220 lines of enum definitions and a decide() function.", "code_understanding", 3),
    ("Is there a race condition in this code?", "Two goroutines increment a shared counter without a mutex, then wg.Wait().", "code_understanding", 3),
    ("What does `git rebase -i HEAD~3` actually do?", "", "code_understanding", 1),
    ("Trace through this recursion for n=4 and tell me the call order.", "def f(n): return 1 if n<=1 else f(n-1)+f(n-2)", "code_understanding", 2),
    ("Why am I getting a borrow checker error on line 14?", "let first = &v[0]; v.push(4); println!(\"{}\", first);", "code_understanding", 2),
    ("Review this PR diff and explain what behavioural change it introduces.", "Changes the default timeout from 30s to None in the HTTP client builder.", "code_understanding", 3),
    ("What does this bash one-liner do? `find . -name '*.log' -mtime +7 -delete`", "", "code_understanding", 1),
    ("Find the bug: the pagination skips the last page when the total is a multiple of the page size.", "pages = total // size + 1 if total % size else total // size", "code_understanding", 3),
    ("Why does this Python function mutate its default argument between calls?", "def add(x, acc=[]): acc.append(x); return acc", "code_understanding", 2),
    ("Explain how this tokio select! loop handles cancellation of the other branch.", "Two branches: a channel recv and a sleep timer, inside loop {}.", "code_understanding", 3),
    ("Can you describe the control flow of this state machine?", "enum State { Idle, Running, Done } with a 40-line transition function.", "code_understanding", 3),
    ("Explain the memory layout of this struct and where padding is inserted.", "#[repr(C)] struct A { a: u8, b: u32, c: u16 }", "code_understanding", 3),
    ("what does `async` do on this function", "async fn load() -> Vec<u8> { ... }", "code_understanding", 1),
    ("Why is this query doing a sequential scan even though there is an index?", "Index on users(email); query uses WHERE lower(email) = $1.", "code_understanding", 3),
    ("Look at this stack trace and tell me what is most likely the root cause.", "NullPointerException at OrderService.java:88 after upgrading the DI container.", "code_understanding", 3),
    # ---------------- technical_design: architecture / design decisions ----------------
    ("Propose a schema for storing multi-tenant audit logs that supports 90-day retention.", "Postgres, ~50M events/day, queries by tenant and time range.", "technical_design", 3),
    ("How should we structure a plugin system for our CLI so third parties can add commands?", "Rust CLI, currently a single crate with clap subcommands.", "technical_design", 3),
    ("Compare Kafka and SQS for our order-event pipeline and recommend one.", "AWS only, 2k msgs/s peak, small team, ordering needed per customer.", "technical_design", 3),
    ("Sketch an architecture for real-time collaborative editing for our notes app.", "Web client, currently REST + Postgres.", "technical_design", 5),
    ("Design the API for a feature-flag service. Include evaluation, targeting rules, and audit.", "", "technical_design", 4),
    ("What is a sensible caching strategy for a read-heavy product catalog?", "10k SKUs, updated hourly, global users.", "technical_design", 2),
    ("Should this be a monolith or microservices?", "Team of 5, 3 domains, one DB, early-stage startup.", "technical_design", 3),
    ("Design a zero-downtime strategy for splitting our users table into two services.", "Postgres, 200M rows, 24/7 traffic.", "technical_design", 5),
    ("Propose a folder structure for a Rust workspace with a library, a CLI, and a server.", "", "technical_design", 1),
    ("How would you design idempotent webhook handling for a payments provider?", "At-least-once delivery, events can arrive out of order.", "technical_design", 4),
    ("Design a sharding scheme for a time-series metrics store.", "Writes 500k points/s, queries are mostly the last 24h.", "technical_design", 5),
    ("Outline an authentication design for a mobile app and a public API sharing one user base.", "", "technical_design", 3),
    ("What data model would you use to represent versioned documents with branching?", "", "technical_design", 3),
    ("Plan the rollout of a breaking API change to 300 integrators.", "Versioning is currently by header; deprecation policy undefined.", "technical_design", 3),
    ("Design a job scheduler that guarantees a task runs at most once across a cluster.", "No ZooKeeper allowed; Postgres is available.", "technical_design", 4),
    ("Where should input validation live in a layered architecture?", "", "technical_design", 2),
    ("Design the observability stack for 40 microservices: logs, metrics, traces, alerting.", "Kubernetes, budget-conscious.", "technical_design", 4),
    ("Suggest an approach for offline-first sync in our mobile app.", "Conflicts are rare; data is per-user.", "technical_design", 4),
    ("Which database fits an event-sourced ledger and why?", "", "technical_design", 2),
    ("Design a multi-region failover plan for our primary database.", "RPO 1 minute, RTO 15 minutes.", "technical_design", 5),
    ("Propose how to split this 5k-line module into cohesive crates.", "Rust; types, parsing, IO and CLI are tangled.", "technical_design", 3),
    ("How would you architect the permission model for nested organisations and projects?", "", "technical_design", 4),
    # ---------------- analytical_reasoning: diagnosis, estimation, multi-step logic ----------------
    ("Our p99 latency doubled after the release but p50 is flat. What are the most likely causes and how do I rank them?", "Release added a feature flag lookup and a new DB index.", "analytical_reasoning", 4),
    ("Estimate how many servers we need for 2M daily active users.", "Avg 40 requests/user/day, 3x peak factor, 20ms CPU per request.", "analytical_reasoning", 3),
    ("A/B test shows +2% conversion with p=0.08. Should we ship?", "n=12,000 per arm, 2 weeks, one holiday weekend inside the window.", "analytical_reasoning", 3),
    ("If a train leaves at 3pm at 60 mph and another at 4pm at 80 mph, when does the second catch up?", "", "analytical_reasoning", 2),
    ("Why might our model's accuracy drop in production while offline validation stays high?", "", "analytical_reasoning", 3),
    ("Which of these three vendors is cheapest over 3 years given the usage curve?", "Vendor A flat 40k/yr; B 0.02 per call with 500k free; C tiered. Usage grows 8% monthly from 2M calls.", "analytical_reasoning", 4),
    ("Work out the expected value of this bet.", "Win 5 with probability 0.3, lose 2 otherwise.", "analytical_reasoning", 1),
    ("Our queue depth grows every Monday morning. Give me a hypothesis tree and the data I'd check first.", "", "analytical_reasoning", 4),
    ("Compare the failure modes of two-phase commit versus sagas for our checkout flow and tell me which risks dominate.", "", "analytical_reasoning", 4),
    ("Is it a coincidence that the errors started right after the DST change?", "Cron jobs in two services; one uses UTC, one local time.", "analytical_reasoning", 3),
    ("Prove that the sum of two even numbers is even.", "", "analytical_reasoning", 1),
    ("Reason about whether this lock ordering can deadlock.", "Thread A locks X then Y; thread B locks Y then X only on the error path.", "analytical_reasoning", 3),
    ("Why did revenue fall 6% last quarter? Decompose it into volume, price and mix effects.", "Table of product-level units and prices per quarter attached.", "analytical_reasoning", 4),
    ("Analyse the trade-offs of switching our CI to self-hosted runners given the numbers.", "Current spend 9k/month, 40 engineers, avg build 12 minutes.", "analytical_reasoning", 3),
    ("Given these logs, reconstruct the sequence of events that led to the double shipment.", "Three services' logs with timestamps, clock skew up to 2s.", "analytical_reasoning", 5),
    ("What are the odds of rolling at least one six in four dice rolls?", "", "analytical_reasoning", 1),
    ("Evaluate whether our retry policy could amplify an outage.", "3 retries, no backoff, 10 upstream callers.", "analytical_reasoning", 4),
    ("Why does cache hit rate fall as we add more nodes?", "Consistent hashing without virtual nodes, 2 to 8 nodes.", "analytical_reasoning", 3),
    ("Critically assess this claim: 'microservices always reduce deployment risk'.", "", "analytical_reasoning", 3),
    ("Explain the statistical flaw in judging the campaign by comparing it to last month.", "", "analytical_reasoning", 2),
    # ---------------- writing: prose, rewriting, summarising ----------------
    ("Draft an email to my team announcing the Friday deploy freeze.", "", "writing", 1),
    ("Write a short, friendly apology to a customer whose order shipped late.", "Order delayed 4 days by carrier.", "writing", 1),
    ("Proofread this paragraph for grammar and tone.", "Our team have been working hard to deliver the features what you requested.", "writing", 1),
    ("Write a blog post introduction about why observability matters for small teams.", "Audience: engineering managers. ~150 words.", "writing", 2),
    ("Turn these bullet points into a polished project update for executives.", "Shipped auth v2; latency -30%; two incidents; hiring 2 SREs.", "writing", 3),
    ("Write release notes for version 2.4 from this changelog.", "feat: bulk import; fix: crash on empty csv; chore: bump deps.", "writing", 2),
    ("Make this README intro more concise and less marketing-heavy.", "Our revolutionary, blazing-fast, next-generation library...", "writing", 2),
    ("Draft a postmortem summary for a non-technical audience.", "30-minute outage, caused by an expired certificate, no data loss.", "writing", 3),
    ("Write a job description for a senior backend engineer.", "Rust, distributed systems, remote-friendly.", "writing", 2),
    ("Give me three catchy names and taglines for a developer tool that compresses prompts.", "", "writing", 2),
    ("Translate this paragraph into Spanish and keep the formal tone.", "Thank you for contacting support. We will reply within one business day.", "writing", 1),
    ("Write a commit message for these changes.", "Adds retry with backoff to the S3 uploader and a test.", "writing", 1),
    ("Write a one-page proposal arguing for adopting trunk-based development.", "Audience: skeptical staff engineers.", "writing", 4),
    ("Polish this LinkedIn post announcing our open-source release.", "", "writing", 1),
    ("Rewrite the following in the active voice.", "The report was reviewed by the committee and the budget was approved.", "writing", 1),
    ("Write a poem about debugging in Python.", "", "writing", 2),
    ("Summarise this meeting transcript into decisions, owners and deadlines.", "45-minute standup transcript pasted below, 3,000 words.", "writing", 3),
    ("Create the speaker notes for a five-minute lightning talk on property-based testing.", "", "writing", 3),
    ("Write a polite reminder email for an overdue invoice.", "Invoice 1042 is 14 days late.", "writing", 1),
    ("Compose a technical blog post comparing two approaches to feature flags, with a clear narrative.", "Audience: mid-level engineers, 1,000 words.", "writing", 4),
    ("Shorten this abstract to 100 words without losing the main result.", "", "writing", 2),
    # ---------------- factual_lookup: short fact/definition retrieval ----------------
    ("What is the capital of Australia?", "", "factual_lookup", 1),
    ("What port does PostgreSQL use by default?", "", "factual_lookup", 1),
    ("Who created the Linux kernel and when?", "", "factual_lookup", 1),
    ("What is the maximum size of an item in DynamoDB?", "", "factual_lookup", 1),
    ("What is the difference between HTTP 401 and 403?", "", "factual_lookup", 1),
    ("When was Rust 1.0 released?", "", "factual_lookup", 1),
    ("What does CORS stand for?", "", "factual_lookup", 1),
    ("How many bits are in an IPv6 address?", "", "factual_lookup", 1),
    ("Which HTTP method is idempotent, PUT or POST?", "", "factual_lookup", 1),
    ("What is the default branch name git uses in new repos?", "", "factual_lookup", 1),
    ("What is the time complexity of binary search?", "", "factual_lookup", 1),
    ("Who wrote 'The Mythical Man-Month'?", "", "factual_lookup", 1),
    ("What does the `-r` flag do in `cp`?", "", "factual_lookup", 1),
    ("What is the boiling point of water at sea level in Fahrenheit?", "", "factual_lookup", 1),
    ("What is the current LTS version of Node.js?", "", "factual_lookup", 1),
    ("Define eventual consistency in one line.", "", "factual_lookup", 1),
    ("What is the RFC number for HTTP/1.1 semantics?", "", "factual_lookup", 2),
    ("Which AWS service offers managed Kafka?", "", "factual_lookup", 1),
    ("How many days are in a leap year?", "", "factual_lookup", 1),
    ("What is the syntax for a Python list comprehension?", "", "factual_lookup", 1),
    ("Who is the author of the `serde` crate?", "", "factual_lookup", 1),
    ("What is the formula for compound interest?", "", "factual_lookup", 1),
    ("Which year did the HTTP/2 specification get published?", "", "factual_lookup", 1),
    # ---------------- general: chit-chat, vague, meta ----------------
    ("hello there", "", "general", 1),
    ("Thanks, that was helpful!", "", "general", 1),
    ("Can you help me with something?", "", "general", 1),
    ("What can you do?", "", "general", 1),
    ("I'm feeling a bit stuck today.", "", "general", 1),
    ("Tell me something interesting.", "", "general", 1),
    ("Good morning! How are you?", "", "general", 1),
    ("ok", "", "general", 1),
    ("Can we continue where we left off?", "", "general", 1),
    ("Which is better, tabs or spaces?", "", "general", 1),
    ("Give me a motivational quote for Monday.", "", "general", 1),
    ("I need some advice on organizing my week.", "", "general", 2),
    ("Plan a weekend trip to Lisbon for two people.", "", "general", 3),
    ("What should I cook tonight with chicken and rice?", "", "general", 1),
    ("Brainstorm ideas for a team offsite activity.", "", "general", 2),
    ("Can you recommend a good book on leadership?", "", "general", 1),
    ("Could you repeat that more simply?", "", "general", 1),
    ("Help me decide between two job offers.", "One pays more, the other has better growth.", "general", 3),
    ("Let's play a word game.", "", "general", 1),
    ("hmm not sure what I want to ask", "", "general", 1),
    ("How do I stay focused when working from home?", "", "general", 2),
    ("Tell me a joke about programmers.", "", "general", 1),
    ("What's a good name for my cat?", "", "general", 1),
    ("Summarize your capabilities in a sentence.", "", "general", 1),
]


# Second batch: regex near-misses (keyword present, different intent), multi-intent (labelled by the
# primary deliverable), noisy/padded and out-of-distribution queries.
SEEDS += [
    # near-misses: "what is" / "who is" style openers that are NOT trivia
    ("What is the best way to structure error handling across 12 microservices that share a message bus?", "", "technical_design", 4),
    ("What is this function doing with the lifetimes here?", "fn longest<'a>(a: &'a str, b: &'a str) -> &'a str { if a.len() > b.len() { a } else { b } }", "code_understanding", 2),
    ("What is causing the memory to grow after each request in this handler?", "Go service, goroutines spawned per request, a map caches results forever.", "analytical_reasoning", 3),
    ("What are the tradeoffs between optimistic and pessimistic locking for our booking system?", "High contention on popular slots.", "technical_design", 3),
    ("Who should own the database schema in a team of three squads sharing one Postgres?", "", "technical_design", 3),
    ("When should I use a trait object instead of generics in Rust?", "", "factual_lookup", 2),
    ("Where is the retry logic implemented in this codebase?", "Monorepo with 40 crates.", "code_understanding", 2),
    # near-misses: "refactor", "architecture", "calculate", "draft", "write" with other intents
    ("Our architecture docs are outdated. Rewrite the overview section to match the new event-driven design.", "Current text describes the old monolith, 400 words.", "writing", 3),
    ("Draft a design doc outline for moving to event sourcing. Just headings, no content.", "", "technical_design", 2),
    ("Should we refactor now or ship first? Weigh the risks given the deadline.", "Two weeks to launch, 30% test coverage.", "analytical_reasoning", 3),
    ("Calculate the cheapest way to cover this: 3 services, 2M requests each, per-request price differs by region.", "", "analytical_reasoning", 3),
    ("Write a short story about a compiler that learns to feel.", "", "writing", 2),
    ("Write an email explaining how the new rate limiter works to our customers.", "Limit is 100 requests per minute per key; bursts allowed up to 20.", "writing", 2),
    ("Write a function-level docstring for this code.", "def normalize(v): return v / (np.linalg.norm(v) or 1)", "writing", 1),
    ("Explain what a mutex is to a ten year old.", "", "writing", 2),
    ("Explain how this code works and then rewrite it to be faster.", "def dedupe(xs): return [x for i,x in enumerate(xs) if x not in xs[:i]]", "code_generation", 3),
    ("Explain why we picked Postgres over Mongo, as a decision record.", "", "writing", 3),
    ("Define a REST API for managing library books, including endpoints and error codes.", "", "technical_design", 3),
    ("Define a Rust struct for a user with an optional email and a list of roles.", "", "code_generation", 1),
    ("Define idempotency.", "", "factual_lookup", 1),
    ("How many requests per second can a single Redis instance handle roughly?", "", "factual_lookup", 2),
    ("How many servers do I need if each handles 800 rps and I expect 12k rps with 30% headroom?", "", "analytical_reasoning", 2),
    ("Solve this: if f(x) = 3x^2 - 12x + 9, find the minimum and where it occurs.", "", "analytical_reasoning", 2),
    ("Prove by induction that 1+2+...+n = n(n+1)/2.", "", "analytical_reasoning", 2),
    ("Fix this bug: the discount is applied twice when the coupon and the loyalty tier overlap.", "def total(cart): d = coupon(cart) + loyalty(cart); return price(cart) - d - coupon(cart)", "code_generation", 3),
    ("fix the bug where the app crashes if the user list is empty", "", "code_generation", 2),
    ("Why is this code slow, and how would you restructure it?", "Nested loops over two 100k-element lists to find matches.", "code_understanding", 3),
    # multi-intent: label by primary deliverable
    ("Summarize this function and then write tests for it.", "def parse_duration(s): ...  # supports 1h30m, 45s, 2d", "code_generation", 3),
    ("Explain the bug in this snippet, then give me the corrected version.", "for (i = 0; i <= arr.length; i++) sum += arr[i];", "code_understanding", 2),
    ("Give me a quick overview of OAuth2 and then draft the login flow for our app.", "SPA plus Rust API.", "technical_design", 3),
    ("Look at these latency numbers, decide if it is a regression, and write a two-line Slack update.", "p50 120ms -> 135ms, p99 400ms -> 900ms after release 4.2.", "analytical_reasoning", 3),
    ("Compare these two designs and then summarize your recommendation in an email to the VP.", "Design A: single Postgres. Design B: Postgres plus a Kafka-fed read model.", "technical_design", 4),
    ("Translate this SQL to a Rust sqlx query and explain what the join does.", "SELECT u.id, COUNT(o.id) FROM users u LEFT JOIN orders o ON o.user_id=u.id GROUP BY u.id", "code_generation", 2),
    # noisy / padded / OOD
    ("pls help!!! my code no work, it say undefined is not a function lol", "button.onclick = handlr;", "code_understanding", 2),
    ("so basically i was wondering, like, whats the diff between a process and a thread", "", "factual_lookup", 1),
    ("URGENT: need a regex for UK postcodes asap thx", "", "code_generation", 2),
    ("write something nice for my coworker's farewell card, she is moving to Berlin", "", "writing", 1),
    ("can u make this sound more professional: hey guys the server is down again, working on it", "", "writing", 1),
    ("tell me why my sourdough starter smells like acetone", "", "general", 2),
    ("What should I name my open source CLI that renames photos by date?", "", "general", 2),
    ("How do I convince my manager to let us pay down tech debt?", "", "general", 3),
    ("is it worth learning Rust in 2026 or should I stick with Go for backend jobs", "", "general", 2),
    ("Recommend a few podcasts about distributed systems.", "", "general", 1),
    ("Who won the 2018 FIFA World Cup?", "", "factual_lookup", 1),
    ("Convert 72 degrees Fahrenheit to Celsius.", "", "factual_lookup", 1),
    ("What's the sum of the first 20 odd numbers?", "", "analytical_reasoning", 1),
    ("what year did the berlin wall fall", "", "factual_lookup", 1),
    ("Explain the difference between TCP and UDP.", "", "factual_lookup", 2),
    ("I have 3 hours before a demo. Tell me what to cut from this plan so it still works.", "Plan lists auth, dashboard, export, email alerts, dark mode, audit log.", "analytical_reasoning", 3),
    ("Lay out how you would test a payments reconciliation job end to end.", "Nightly batch, reads provider CSVs, writes to ledger.", "technical_design", 3),
    ("Give me a quick Python one-liner to flatten a list of lists.", "", "code_generation", 1),
    ("What would break if we changed this enum's discriminants?", "#[repr(u8)] enum Op { Add = 1, Sub = 2 } is serialized to disk and sent over the wire.", "analytical_reasoning", 3),
    ("Rewrite this error message so it tells the user what to do next.", "Error: ECONNREFUSED 127.0.0.1:5432", "writing", 1),
    ("List the pros and cons of using GraphQL for a mobile backend.", "", "technical_design", 2),
    ("thanks! one more thing, can you also check the spelling in my cover letter", "", "writing", 1),
    ("What is 17 times 23?", "", "analytical_reasoning", 1),
    ("Create a table of HTTP status codes grouped by class.", "", "factual_lookup", 1),
    ("Help me understand this error: cannot move out of borrowed content.", "", "code_understanding", 2),
    ("Set up a CI pipeline that builds, tests, and deploys a Rust service to Fly.io on tag push.", "", "code_generation", 3),
    ("Write a Python script that reads a folder of PDFs, extracts the text, and saves a CSV of word counts.", "", "code_generation", 3),
    ("Is this SQL injection safe?", "query = f\"SELECT * FROM users WHERE name = '{name}'\"", "code_understanding", 2),
    ("Describe the architecture of our ingestion pipeline for the new hire onboarding doc.", "Kafka -> Flink -> ClickHouse, with a dead-letter topic.", "writing", 3),
    ("Review my approach: I plan to put the feature flags in the same table as user settings.", "", "technical_design", 2),
    ("ok", "", "general", 1),
    ("hmm", "Previous message asked about caching.", "general", 1),
    ("Which one would you pick?", "", "general", 1),
    ("Could be either honestly. Thoughts?", "", "general", 1),
    ("Write a haiku about merge conflicts.", "", "writing", 1),
    ("Analyse these quarterly figures and tell me whether the churn increase is statistically meaningful.", "Q1 churn 2.1% of 40k; Q2 churn 2.4% of 42k.", "analytical_reasoning", 3),
    ("Describe the steps to roll back a failed database migration safely.", "Postgres, migration added a NOT NULL column with a default.", "technical_design", 3),
]

PAD_CONTEXT = [
    "Please be concise.",
    "This is for an internal team wiki.",
    "I'm a beginner, so keep it simple.",
    "Sent from my phone, sorry for typos.",
    "FYI this is not urgent.",
    "Our stack is mostly Python and Postgres on AWS.",
    "Thanks in advance!",
]
LEAD_NOISE = ["hey, ", "quick question: ", "Hi team, ", "um so ", "Hello! ", "ok so ", "Please help: "]


def variants(query: str, context: str, rng: random.Random):
    """Deterministic surface variants that keep the label: casing, filler, padding, typos."""
    out = [(query, context)]
    out.append((rng.choice(LEAD_NOISE) + query[0].lower() + query[1:], context))
    out.append((query.rstrip(".?!").lower(), context))
    out.append((query, (context + " " + rng.choice(PAD_CONTEXT)).strip()))
    words = query.split()
    if len(words) > 4:
        i = rng.randrange(len(words))
        w = words[i]
        if len(w) > 3 and w.isalpha():
            words[i] = w[:2] + w[3] + w[2] + w[4:]  # transposition typo
        out.append((" ".join(words), context))
    return out


def write_seed_eval(out_dir: Path) -> None:
    """All original (un-augmented) seeds in the public eval format, for scoring model-free backends
    (regex, hosted LLM) on the same cases the grouped cross-validation uses."""
    examples = [
        {"id": f"seed-{i:03d}", "query": q, "context": c, "request_type": t, "complexity": cx}
        for i, (q, c, t, cx) in enumerate(SEEDS)
    ]
    (out_dir / "seeds_eval.json").write_text(json.dumps({"examples": examples}, indent=1))
    print("seeds_eval.json", len(examples), "cases")


def main() -> None:
    rng = random.Random(1234)
    out_dir = Path(__file__).parent / "data"
    out_dir.mkdir(exist_ok=True)

    # Split by seed (grouped) and stratify by class so val has every label.
    by_label: dict[str, list[int]] = {}
    for i, s in enumerate(SEEDS):
        by_label.setdefault(s[2], []).append(i)
    val_seeds: set[int] = set()
    for idxs in by_label.values():
        shuffled = idxs[:]
        rng.shuffle(shuffled)
        val_seeds.update(shuffled[: max(3, round(len(idxs) * 0.25))])

    splits = {"train": [], "val": []}
    for i, (q, ctx, label, cx) in enumerate(SEEDS):
        split = "val" if i in val_seeds else "train"
        vs = variants(q, ctx, rng) if split == "train" else [(q, ctx)]  # val stays un-augmented seeds
        for vq, vctx in vs:
            splits[split].append(
                {"seed": i, "query": vq, "context": vctx, "request_type": label, "complexity": cx}
            )
    write_seed_eval(out_dir)
    for name, rows in splits.items():
        with open(out_dir / f"{name}.jsonl", "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
        print(name, len(rows), "rows from", len({r["seed"] for r in rows}), "seeds")


if __name__ == "__main__":
    main()
