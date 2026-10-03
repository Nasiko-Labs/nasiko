"""
Dataset generator for P2 Request Classifier and Complexity Classifier.

Generates ~300 base examples + ~130 derived/perturbed examples (~430 total).
Strictly groups examples by source_group to prevent data leakage across splits.
Splits: train (70%), calibration (15%), validation (15%).
"""

import json
import random
import re

# Base examples definition: list of dicts:
# {
#   "group": str,
#   "query": str,
#   "context": str | None,
#   "request_type": str,
#   "complexity": int,
#   "perturbations": list of dicts or templates
# }

RAW_DATA = [
    # ==========================================
    # 1. CODE GENERATION (Complexity 1 - 5)
    # ==========================================
    # Regex near-misses & standard
    {
        "group": "cg_01",
        "query": "Write a python function to check if a string is a palindrome.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 1,
        "derived": [
            ("make a python function that tells if a word is palindrome or not", None, 1),
            ("writ a pythn functin to chek if string is palindrom", None, 1), # typo
            ("Hi assistant! Please write a python function to check if a string is a palindrome. Much appreciated!", None, 1), # padded
        ]
    },
    {
        "group": "cg_02",
        "query": "Implement binary search on a sorted integer slice in Rust.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 2,
        "derived": [
            ("implement binary search over a sorted slice in rust", None, 2),
            ("impliment bynary serch on a sortd int slice in rust", None, 2),
        ]
    },
    {
        "group": "cg_03",
        "query": "Create a FastAPI endpoint that accepts JSON user profiles and validates email using pydantic.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 2,
        "derived": [
            ("Write a REST API endpoint in FastAPI for user signup validation.", None, 2),
        ]
    },
    {
        "group": "cg_04",
        "query": "Write an LRU Cache in Go with thread-safe Get and Put methods using a doubly linked list and mutex.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 3,
        "derived": [
            ("implement a thread-safe LRU cache in golang with sync.Mutex", None, 3),
        ]
    },
    {
        "group": "cg_05",
        "query": "Implement a distributed rate limiter in Python using Redis sliding window log with Lua script for atomicity.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 4,
        "derived": [
            ("Write a redis lua sliding window rate limiter function in python.", None, 4),
        ]
    },
    {
        "group": "cg_06",
        "query": "Build a multi-producer multi-consumer bounded lock-free ring buffer in C++20 using atomic memory orders (acquire/release).",
        "context": None,
        "request_type": "code_generation",
        "complexity": 5,
        "derived": [
            ("write lock-free bounded mpmc queue in cpp using std::atomic with strict memory ordering", None, 5),
        ]
    },
    {
        "group": "cg_07",
        "query": "Fix this bug: the loop index goes out of bounds on empty arrays.",
        "context": "fn process(items: &[i32]) -> i32 { items[0] + items[items.len()] }",
        "request_type": "code_generation",
        "complexity": 2,
        "derived": [
            ("Fix the off-by-one and empty check bug in this function", "fn process(items: &[i32]) -> i32 { items[0] + items[items.len()] }", 2),
        ]
    },
    {
        "group": "cg_08",
        "query": "Refactor this synchronous HTTP handler into an asynchronous non-blocking Tokio handler.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 3,
    },
    {
        "group": "cg_09",
        "query": "Add comprehensive error handling and retry logic with exponential backoff to this DB connection pool.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 3,
    },
    {
        "group": "cg_10",
        "query": "Write a SQL query to find top 3 customers by revenue per region using window functions.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 2,
    },
    {
        "group": "cg_11",
        "query": "Generate a TypeScript interface and zod schema for a GitHub webhook payload.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 2,
    },
    {
        "group": "cg_12",
        "query": "Write a bash one-liner to find all files larger than 100MB and compress them with gzip.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 1,
    },
    {
        "group": "cg_13",
        "query": "Implement A* pathfinding algorithm on a 2D grid in Java with custom Manhattan distance heuristic.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 3,
    },
    {
        "group": "cg_14",
        "query": "Write a custom PyTorch training loop with gradient accumulation, mixed precision amp, and wandb logging.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 4,
    },
    {
        "group": "cg_15",
        "query": "Generate a Kubernetes Helm chart for a stateful PostgreSQL cluster with persistence and automated backup cronjob.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 4,
    },
    {
        "group": "cg_16",
        "query": "Build a zero-copy parser in Rust using nom for the Redis RESP3 serialization protocol.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 4,
    },
    {
        "group": "cg_17",
        "query": "Implement Raft consensus leader election and log replication module in Rust with simulated RPC transports.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 5,
    },
    {
        "group": "cg_18",
        "query": "Write a quicksort function in C.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 2,
    },
    {
        "group": "cg_19",
        "query": "Create a React hook `useDebounce` that delays updating value until 300ms of inactivity.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 2,
    },
    {
        "group": "cg_20",
        "query": "Write a Python script that parses CSV rows and sends batch email notifications via SMTP.",
        "context": None,
        "request_type": "code_generation",
        "complexity": 2,
    },

    # ==========================================
    # 2. CODE UNDERSTANDING (Complexity 1 - 5)
    # ==========================================
    # Minimal pair with code generation:
    {
        "group": "cu_01",
        "query": "Explain what this function does and how it handles null pointers.",
        "context": "void process(Node* n) { if(!n) return; free(n->val); }",
        "request_type": "code_understanding",
        "complexity": 2,
        "derived": [
            ("what does this function do and how does it deal with null pointers?", "void process(Node* n) { if(!n) return; free(n->val); }", 2),
            ("explan what this code is doing with null pointers", "void process(Node* n) { if(!n) return; free(n->val); }", 2),
        ]
    },
    {
        "group": "cu_02",
        "query": "Explain why this function is slow and what causes high memory allocations.",
        "context": "def process(items): return [str(x) for x in range(len(items)) if str(x) in items]",
        "request_type": "code_understanding",
        "complexity": 2,
        "derived": [
            ("why is this python function slow? analyze the computational complexity", "def process(items): return [str(x) for x in range(len(items)) if str(x) in items]", 2),
        ]
    },
    {
        "group": "cu_03",
        "query": "Walk me through this Rust async code and explain why deadlocks occur on these locks.",
        "context": "async fn deadlocking() { let a = lock1.lock().await; let b = lock2.lock().await; }",
        "request_type": "code_understanding",
        "complexity": 3,
    },
    {
        "group": "cu_04",
        "query": "What does the `volatile` keyword mean in C in the context of embedded memory mapped IO?",
        "context": None,
        "request_type": "code_understanding",
        "complexity": 2,
    },
    {
        "group": "cu_05",
        "query": "How does the borrow checker in Rust ensure memory safety without a garbage collector?",
        "context": None,
        "request_type": "code_understanding",
        "complexity": 3,
    },
    {
        "group": "cu_06",
        "query": "Analyze this SQL execution plan and tell me why the optimizer chose a sequential scan over the index.",
        "context": "Seq Scan on orders (cost=0.00..1845.00 rows=50000 width=72) Filter: (status = 'PENDING')",
        "request_type": "code_understanding",
        "complexity": 3,
    },
    {
        "group": "cu_07",
        "query": "Explain how garbage collection works in Java, specifically comparing G1GC and ZGC mechanics.",
        "context": None,
        "request_type": "code_understanding",
        "complexity": 4,
    },
    {
        "group": "cu_08",
        "query": "Deconstruct this assembly snippet and explain how the stack frame is initialized and restored.",
        "context": "push rbp; mov rbp, rsp; sub rsp, 32; ...; leave; ret",
        "request_type": "code_understanding",
        "complexity": 4,
    },
    {
        "group": "cu_09",
        "query": "Explain the formal semantics of C++ memory_order_consume vs memory_order_acquire on weakly ordered architectures like ARM and Alpha.",
        "context": None,
        "request_type": "code_understanding",
        "complexity": 5,
    },
    {
        "group": "cu_10",
        "query": "What is the difference between `interface{}` and `any` in modern Go?",
        "context": None,
        "request_type": "code_understanding",
        "complexity": 1,
    },
    {
        "group": "cu_11",
        "query": "How does React fiber reconciliation schedule priority updates and pause execution?",
        "context": None,
        "request_type": "code_understanding",
        "complexity": 4,
    },
    {
        "group": "cu_12",
        "query": "Review this cryptographic signing function and identify the side-channel timing attack vulnerability.",
        "context": "for i in range(len(sig)): if sig[i] != expected[i]: return False",
        "request_type": "code_understanding",
        "complexity": 4,
    },
    {
        "group": "cu_13",
        "query": "What is the time and space complexity of merge sort vs heap sort?",
        "context": None,
        "request_type": "code_understanding",
        "complexity": 2,
    },
    {
        "group": "cu_14",
        "query": "Explain what this regex pattern does: `^(?=.*[A-Za-z])(?=.*\\d)[A-Za-z\\d]{8,}$`.",
        "context": None,
        "request_type": "code_understanding",
        "complexity": 2,
    },
    {
        "group": "cu_15",
        "query": "Explain why this TypeScript generic condition is evaluating to `never` instead of the union type.",
        "context": "type Flatten<T> = T extends (infer U)[] ? U : never;",
        "request_type": "code_understanding",
        "complexity": 3,
    },

    # ==========================================
    # 3. TECHNICAL DESIGN (Complexity 1 - 5)
    # ==========================================
    {
        "group": "td_01",
        "query": "Design a relational database schema for a simple blog with users, posts, and comments.",
        "context": None,
        "request_type": "technical_design",
        "complexity": 2,
        "derived": [
            ("how should i design a database schema for blog posts and user comments?", None, 2),
            ("design database tables for users, posts, tags and comments in postgres", None, 2),
        ]
    },
    {
        "group": "td_02",
        "query": "How should I design an API rate limiter to protect our backend services from DDoS?",
        "context": None,
        "request_type": "technical_design",
        "complexity": 3,
        "derived": [
            ("api design for global rate limiting gateway", None, 3),
        ]
    },
    {
        "group": "td_03",
        "query": "Design the system architecture for a real-time collaborative document editor like Google Docs.",
        "context": None,
        "request_type": "technical_design",
        "complexity": 4,
        "derived": [
            ("system architecture for collaborative text editing with CRDTs and websockets", None, 4),
        ]
    },
    {
        "group": "td_04",
        "query": "What are the trade-offs between Apache Kafka and RabbitMQ for event-driven microservices?",
        "context": None,
        "request_type": "technical_design",
        "complexity": 3,
    },
    {
        "group": "td_05",
        "query": "Design a globally distributed payment processing system handling 100k TPS with strict ACID guarantees, multi-region active-active failover, and zero transaction loss.",
        "context": None,
        "request_type": "technical_design",
        "complexity": 5,
        "derived": [
            ("Architecture for global multi-region active-active payment gateway with idempotent ledger and two-phase commit or saga pattern", None, 5),
        ]
    },
    {
        "group": "td_06",
        "query": "Design a URL shortener service like Bitly including data model, hashing strategy, and caching layer.",
        "context": None,
        "request_type": "technical_design",
        "complexity": 3,
    },
    {
        "group": "td_07",
        "query": "How should we architect an authentication service supporting OAuth2, OIDC, SAML, and MFA for enterprise tenants?",
        "context": None,
        "request_type": "technical_design",
        "complexity": 4,
    },
    {
        "group": "td_08",
        "query": "Compare monotonic IDs (UUIDv7, ULID, Snowflake) vs auto-incrementing integers for distributed databases.",
        "context": None,
        "request_type": "technical_design",
        "complexity": 3,
    },
    {
        "group": "td_09",
        "query": "Propose an observability architecture using OpenTelemetry, Prometheus, Jaeger, and Grafana for a Kubernetes cluster.",
        "context": None,
        "request_type": "technical_design",
        "complexity": 4,
    },
    {
        "group": "td_10",
        "query": "Design a fault-tolerant notification pipeline delivering push notifications, emails, and SMS with provider failover and rate limits.",
        "context": None,
        "request_type": "technical_design",
        "complexity": 3,
    },
    {
        "group": "td_11",
        "query": "What are the architectural trade-offs of micro frontends vs monolithic single page applications?",
        "context": None,
        "request_type": "technical_design",
        "complexity": 3,
    },
    {
        "group": "td_12",
        "query": "Design an enterprise data mesh architecture across multiple cloud providers with federated governance and lineage tracking.",
        "context": None,
        "request_type": "technical_design",
        "complexity": 5,
    },
    {
        "group": "td_13",
        "query": "How should I structure the folder layout for a clean architecture Go project?",
        "context": None,
        "request_type": "technical_design",
        "complexity": 2,
    },

    # ==========================================
    # 4. ANALYTICAL REASONING (Complexity 1 - 5)
    # ==========================================
    {
        "group": "ar_01",
        "query": "Calculate the compound interest on $5,000 invested at an annual interest rate of 6% compounded monthly for 4 years.",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 2,
        "derived": [
            ("calculate the interest for 5000 dollars at 6% monthly compound after 4 years", None, 2),
            ("what is 5000 * (1 + 0.06/12)**(12*4)?", None, 2),
        ]
    },
    {
        "group": "ar_02",
        "query": "What is the probability of rolling a sum of 8 with two fair six-sided dice?",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 1,
        "derived": [
            ("probability of rolling 8 with two dice", None, 1),
        ]
    },
    {
        "group": "ar_03",
        "query": "Solve this system of linear equations: 3x + 2y - z = 1, 2x - 2y + 4z = -2, -x + 0.5y - z = 0.",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 3,
    },
    {
        "group": "ar_04",
        "query": "Prove that the square root of 2 is an irrational number by contradiction.",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 3,
        "derived": [
            ("prove by contradiction that sqrt(2) is irrational", None, 3),
        ]
    },
    {
        "group": "ar_05",
        "query": "Three travelers check into a hotel room costing $30 ($10 each). The clerk realizes the room was only $25 and gives $5 to the bellboy to return. The bellboy keeps $2 and returns $1 to each traveler. Now each paid $9 ($27 total) plus $2 the bellboy kept = $29. Where is the missing dollar?",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 2,
    },
    {
        "group": "ar_06",
        "query": "Calculate the Nash equilibrium in a two-player asymmetric game with payoff matrix [[(3,1), (0,0)], [(0,0), (1,3)]].",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 4,
    },
    {
        "group": "ar_07",
        "query": "Derive the closed-form Black-Scholes formula for European call option pricing using risk-neutral expectation and geometric Brownian motion.",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 5,
    },
    {
        "group": "ar_08",
        "query": "What's the sum of 458 and 792?",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 1,
    },
    {
        "group": "ar_09",
        "query": "How many ways can 5 people be seated at a round table if two particular people refuse to sit next to each other?",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 2,
    },
    {
        "group": "ar_10",
        "query": "Prove that every planar graph is 5-colorable using induction and Euler's formula.",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 4,
    },
    {
        "group": "ar_11",
        "query": "If a train travels 60 miles at 30 mph and returns the same distance at 60 mph, what is its average speed for the round trip?",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 2,
    },
    {
        "group": "ar_12",
        "query": "Derive the optimal policy for a Markov Decision Process with state transition probabilities P and discount factor gamma using policy iteration.",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 5,
    },
    {
        "group": "ar_13",
        "query": "Calculate the eigenvalues and eigenvectors of matrix [[4, -2], [1, 1]].",
        "context": None,
        "request_type": "analytical_reasoning",
        "complexity": 3,
    },

    # ==========================================
    # 5. WRITING (Complexity 1 - 5)
    # ==========================================
    # Regex near-misses (e.g. mention python or code)
    {
        "group": "wr_01",
        "query": "Write a poem about Python programming and the elegance of indentation.",
        "context": None,
        "request_type": "writing",
        "complexity": 2,
        "derived": [
            ("compose a short poem about python scripts and coding in the night", None, 2),
            ("writ a peom about python", None, 2), # typo
            ("Could you please write a poem about Python? It's for my team's newsletter. Thank you!", None, 2),
        ]
    },
    {
        "group": "wr_02",
        "query": "Draft an email to stakeholders explaining that the production database outage has been resolved.",
        "context": None,
        "request_type": "writing",
        "complexity": 2,
        "derived": [
            ("write an email update to executives regarding the database outage resolution", None, 2),
        ]
    },
    {
        "group": "wr_03",
        "query": "Rewrite this paragraph to make it sound more professional and concise for an executive memo.",
        "context": "Basically our team did a lot of things last month and we got stuff done on time although there was some problems.",
        "request_type": "writing",
        "complexity": 2,
    },
    {
        "group": "wr_04",
        "query": "Compose a persuasive cover letter for a Senior Site Reliability Engineer role at Stripe.",
        "context": None,
        "request_type": "writing",
        "complexity": 3,
    },
    {
        "group": "wr_05",
        "query": "Draft a detailed post-mortem report for a multi-hour cloud regional outage including timeline, root cause analysis, impact, and preventive action items.",
        "context": None,
        "request_type": "writing",
        "complexity": 4,
    },
    {
        "group": "wr_06",
        "query": "Write a press release announcing our series B funding round of $40 million led by Andreessen Horowitz.",
        "context": None,
        "request_type": "writing",
        "complexity": 3,
    },
    {
        "group": "wr_07",
        "query": "Write a comprehensive white paper explaining the impact of European AI Act compliance on enterprise fintech pipelines.",
        "context": None,
        "request_type": "writing",
        "complexity": 5,
    },
    {
        "group": "wr_08",
        "query": "Draft a polite slack message declining an invitation to a meeting due to a deadline conflict.",
        "context": None,
        "request_type": "writing",
        "complexity": 1,
    },
    {
        "group": "wr_09",
        "query": "Rewrite this product description to adopt an inspiring and energetic marketing tone.",
        "context": "Our shoe is light and has rubber soles for running.",
        "request_type": "writing",
        "complexity": 2,
    },
    {
        "group": "wr_10",
        "query": "Compose an essay analyzing the themes of isolation and technological alienation in 20th century dystopian literature.",
        "context": None,
        "request_type": "writing",
        "complexity": 4,
    },
    {
        "group": "wr_11",
        "query": "Write a script for a 60-second video advertisement introducing an eco-friendly water bottle.",
        "context": None,
        "request_type": "writing",
        "complexity": 2,
    },
    {
        "group": "wr_12",
        "query": "Draft terms of service and acceptable use policy for a SaaS API platform.",
        "context": None,
        "request_type": "writing",
        "complexity": 4,
    },

    # ==========================================
    # 6. FACTUAL LOOKUP (Complexity 1 - 5)
    # ==========================================
    {
        "group": "fl_01",
        "query": "What is the capital of Australia?",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 1,
        "derived": [
            ("what is the capital city of australia", None, 1),
            ("wat is the captal of austrlia?", None, 1),
        ]
    },
    {
        "group": "fl_02",
        "query": "Who was the first person to walk on the Moon and in what year?",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 1,
        "derived": [
            ("who walked on the moon first and when was it", None, 1),
        ]
    },
    {
        "group": "fl_03",
        "query": "Define the term 'epistemology' in philosophy.",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 1,
    },
    {
        "group": "fl_04",
        "query": "How many states are there in the United States?",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 1,
    },
    {
        "group": "fl_05",
        "query": "When was the Treaty of Westphalia signed and what conflict did it end?",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 2,
    },
    {
        "group": "fl_06",
        "query": "What is the atomic weight and electron configuration of Uranium-238?",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 2,
    },
    {
        "group": "fl_07",
        "query": "List all countries bordering the Mediterranean Sea and their official languages.",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 3,
    },
    {
        "group": "fl_08",
        "query": "Provide a comprehensive chronological timeline of the Byzantine-Sasanian War of 602â€“628, including key battles and commanders.",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 4,
    },
    {
        "group": "fl_09",
        "query": "What is the Speed of Light in a vacuum in meters per second?",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 1,
    },
    {
        "group": "fl_10",
        "query": "Who discovered Penicillin and in what year was it discovered?",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 1,
    },
    {
        "group": "fl_11",
        "query": "What is the primary difference between DNA and RNA nucleotide structures?",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 2,
    },
    {
        "group": "fl_12",
        "query": "Provide the complete structural formula and IUPAC naming history of morphine and its synthetic derivatives.",
        "context": None,
        "request_type": "factual_lookup",
        "complexity": 4,
    },

    # ==========================================
    # 7. GENERAL (Complexity 1 - 5)
    # ==========================================
    {
        "group": "gen_01",
        "query": "Hello! How are you doing today?",
        "context": None,
        "request_type": "general",
        "complexity": 1,
        "derived": [
            ("hi there assistant", None, 1),
            ("hey hows it going", None, 1),
        ]
    },
    {
        "group": "gen_02",
        "query": "Can you help me brainstorm some gift ideas for a coffee lover?",
        "context": None,
        "request_type": "general",
        "complexity": 2,
        "derived": [
            ("give me gift suggestions for someone who loves espresso", None, 2),
        ]
    },
    {
        "group": "gen_03",
        "query": "What are some good tips for improving focus and reducing distractions while working from home?",
        "context": None,
        "request_type": "general",
        "complexity": 2,
    },
    {
        "group": "gen_04",
        "query": "I am planning a 7-day trip to Kyoto in autumn. Can you recommend a balanced itinerary?",
        "context": None,
        "request_type": "general",
        "complexity": 3,
    },
    {
        "group": "gen_05",
        "query": "What are the general pros and cons of living in a small town versus a large metropolitan city?",
        "context": None,
        "request_type": "general",
        "complexity": 2,
    },
    {
        "group": "gen_06",
        "query": "Thank you for the explanation, that was very clear!",
        "context": "Assistant: Here is the breakdown of your code.",
        "request_type": "general",
        "complexity": 1,
    },
    {
        "group": "gen_07",
        "query": "Can you summarize the main discussion points from this meeting transcript?",
        "context": "Alice: We need to ship on Friday. Bob: Let's test first.",
        "request_type": "general",
        "complexity": 2,
    },
    {
        "group": "gen_08",
        "query": "Tell me an interesting fun fact about octopuses.",
        "context": None,
        "request_type": "general",
        "complexity": 1,
    },
    {
        "group": "gen_09",
        "query": "Can you provide general advice on preparing for a behavioral interview for a manager role?",
        "context": None,
        "request_type": "general",
        "complexity": 3,
    },
    {
        "group": "gen_10",
        "query": "Good morning. What can you do?",
        "context": None,
        "request_type": "general",
        "complexity": 1,
    },
]

# Additional curated base queries across all classes and complexities to reach ~300 base examples:
ADDITIONAL_DATA = [
    # Code Generation
    ("cg_ext_01", "Write a python script using BeautifulSoup to scrape article titles from a blog.", None, "code_generation", 2),
    ("cg_ext_02", "Implement a red-black tree insertion with rebalancing rotations in Rust.", None, "code_generation", 4),
    ("cg_ext_03", "Write a dockerfile for a multi-stage Rust build with distroless base image.", None, "code_generation", 2),
    ("cg_ext_04", "Create a SQL trigger to automatically update the `updated_at` column on row updates.", None, "code_generation", 2),
    ("cg_ext_05", "Implement Dijkstra's shortest path algorithm using a min-heap in C++.", None, "code_generation", 3),
    ("cg_ext_06", "Write a Python decorator that caches return values with a 60 second TTL.", None, "code_generation", 2),
    ("cg_ext_07", "Build an automated CI/CD GitHub Actions workflow for linting, testing, and pushing docker containers to ECR.", None, "code_generation", 3),
    ("cg_ext_08", "Implement a B-tree indexing page node in C with binary split and search.", None, "code_generation", 5),
    ("cg_ext_09", "Write a regex to validate international phone numbers in E.164 format.", None, "code_generation", 1),
    ("cg_ext_10", "Implement matrix multiplication in WebAssembly using SIMD instructions.", None, "code_generation", 5),
    ("cg_ext_11", "Write a GraphQL resolver in Apollo Server for nested comments pagination.", None, "code_generation", 3),
    ("cg_ext_12", "Implement an undo-redo stack in TypeScript using the Command pattern.", None, "code_generation", 3),
    ("cg_ext_13", "Write a Go CLI tool using cobra that takes flags and writes JSON output.", None, "code_generation", 2),
    ("cg_ext_14", "Implement quicksort in Kotlin with Lomuto partition scheme.", None, "code_generation", 2),
    ("cg_ext_15", "Write a custom allocator in C using `mmap` with a free list strategy.", None, "code_generation", 5),
    ("cg_ext_16", "Refactor this nested callback hell into async/await with try/catch blocks.", None, "code_generation", 2),
    ("cg_ext_17", "Implement merge sort on a singly linked list in C++ without extra space.", None, "code_generation", 3),
    ("cg_ext_18", "Write a Solidity smart contract for an ERC-20 token with minting and burning caps.", None, "code_generation", 3),
    ("cg_ext_19", "Create a PySpark job to calculate rolling 7-day active user counts.", None, "code_generation", 3),
    ("cg_ext_20", "Implement an asynchronous DNS client in Rust using mio and raw UDP sockets.", None, "code_generation", 5),

    # Code Understanding
    ("cu_ext_01", "Explain the difference between call, apply, and bind in JavaScript.", None, "code_understanding", 2),
    ("cu_ext_02", "What causes a stack overflow error in recursive functions and how do compilers optimize tail calls?", None, "code_understanding", 3),
    ("cu_ext_03", "Walk me through how this TLS 1.3 handshake packet exchange works.", None, "code_understanding", 4),
    ("cu_ext_04", "Explain how Python's GIL affects multithreading vs multiprocessing.", None, "code_understanding", 3),
    ("cu_ext_05", "What does the `#[derive(Copy, Clone)]` attribute do in Rust?", None, "code_understanding", 1),
    ("cu_ext_06", "Explain how CSS flexbox `flex-grow`, `flex-shrink`, and `flex-basis` interact.", None, "code_understanding", 2),
    ("cu_ext_07", "How does branch prediction work in modern CPUs and why does sorting an array speed up processing?", None, "code_understanding", 4),
    ("cu_ext_08", "What is the difference between shallow copy and deep copy in Python?", None, "code_understanding", 1),
    ("cu_ext_09", "Walk me through how the Linux epoll syscall handles I/O multiplexing under edge-triggered vs level-triggered modes.", None, "code_understanding", 5),
    ("cu_ext_10", "Explain the difference between optimistic concurrency control and pessimistic locking in databases.", None, "code_understanding", 3),
    ("cu_ext_11", "Why does `0.1 + 0.2 !== 0.3` in floating-point arithmetic?", None, "code_understanding", 1),
    ("cu_ext_12", "Explain the concept of memory leaks in garbage-collected languages.", None, "code_understanding", 2),
    ("cu_ext_13", "Analyze this crash trace: `SIGSEGV in malloc_consolidate` and explain the heap corruption cause.", None, "code_understanding", 4),
    ("cu_ext_14", "Explain the semantics of Rust's `Pin<&mut T>` and why self-referential structs require it.", None, "code_understanding", 4),
    ("cu_ext_15", "How does BGP routing convergence work and what are route flap damping mechanisms?", None, "code_understanding", 4),
    ("cu_ext_16", "What is the difference between process and thread context switching overhead?", None, "code_understanding", 3),
    ("cu_ext_17", "Explain the Raft log compaction mechanism and how snapshots are transferred to slow followers.", None, "code_understanding", 4),
    ("cu_ext_18", "What does `extern \"C\"` do in C++ and why is name mangling disabled?", None, "code_understanding", 2),
    ("cu_ext_19", "Explain the difference between static and dynamic dispatch in object-oriented programming.", None, "code_understanding", 2),
    ("cu_ext_20", "Why does Go channel send deadlock if the buffer is full and no goroutine is reading?", None, "code_understanding", 2),

    # Technical Design
    ("td_ext_01", "Design a database schema for an e-commerce platform with inventory reservations.", None, "technical_design", 3),
    ("td_ext_02", "How should we design a webhook delivery system that guarantees at-least-once delivery with exponential backoff?", None, "technical_design", 3),
    ("td_ext_03", "Architect a high-throughput search autocomplete system with latency under 10ms at p99.", None, "technical_design", 4),
    ("td_ext_04", "What are the trade-offs between REST, gRPC, and GraphQL for internal service-to-service communication?", None, "technical_design", 3),
    ("td_ext_05", "Design a video streaming platform architecture like Netflix including encoding pipeline, CDN distribution, and DRM.", None, "technical_design", 5),
    ("td_ext_06", "How should I design an idempotency key mechanism in an HTTP REST API?", None, "technical_design", 3),
    ("td_ext_07", "Propose a zero-downtime database migration strategy for changing a column type on a table with 500 million rows.", None, "technical_design", 4),
    ("td_ext_08", "Design a metrics aggregation pipeline processing 10 million events per second using Kafka, Flink, and ClickHouse.", None, "technical_design", 5),
    ("td_ext_09", "Compare optimistic locking vs distributed Redis lock for booking airline seats.", None, "technical_design", 3),
    ("td_ext_10", "Design the storage and sharding architecture for a chat messaging application like WhatsApp.", None, "technical_design", 4),
    ("td_ext_11", "How should we design an audit logging architecture to satisfy SOC 2 and GDPR compliance?", None, "technical_design", 3),
    ("td_ext_12", "What are the trade-offs of using event sourcing vs CRUD architecture?", None, "technical_design", 3),
    ("td_ext_13", "Design an automated canary deployment pipeline using Istio and Argo Rollouts in Kubernetes.", None, "technical_design", 4),
    ("td_ext_14", "Architect a decentralized file storage protocol using IPFS and incentives.", None, "technical_design", 5),
    ("td_ext_15", "How should I design a caching hierarchy with L1 in-memory and L2 Redis for a multi-tenant SaaS application?", None, "technical_design", 3),

    # Analytical Reasoning
    ("ar_ext_01", "Calculate the derivative of f(x) = x^3 * e^(2x) using the product rule.", None, "analytical_reasoning", 2),
    ("ar_ext_02", "What is the expected number of coin flips needed to get two consecutive heads?", None, "analytical_reasoning", 3),
    ("ar_ext_03", "Solve this recurrence relation: T(n) = 2T(n/2) + n using the Master Theorem.", None, "analytical_reasoning", 3),
    ("ar_ext_04", "How many integers between 1 and 1000 are divisible by 3 or 5 but not both?", None, "analytical_reasoning", 2),
    ("ar_ext_05", "Prove that there are infinitely many prime numbers using Euclid's proof.", None, "analytical_reasoning", 3),
    ("ar_ext_06", "Calculate the determinant of a 3x3 matrix [[1, 2, 3], [0, 4, 5], [1, 0, 6]].", None, "analytical_reasoning", 2),
    ("ar_ext_07", "Solve the Knapsack problem with weights [2, 3, 4] and values [3, 4, 5] for capacity 5.", None, "analytical_reasoning", 3),
    ("ar_ext_08", "What is 15% tip on a restaurant bill of $84.60?", None, "analytical_reasoning", 1),
    ("ar_ext_09", "Derive the gradient of cross-entropy loss with respect to softmax logits.", None, "analytical_reasoning", 4),
    ("ar_ext_10", "Prove that the Halting problem is undecidable using a diagonal argument.", None, "analytical_reasoning", 4),
    ("ar_ext_11", "If a bag has 4 red balls and 6 blue balls, what is the probability of drawing 2 red balls without replacement?", None, "analytical_reasoning", 2),
    ("ar_ext_12", "Calculate the volume of a sphere with radius 7 cm.", None, "analytical_reasoning", 1),
    ("ar_ext_13", "Solve the differential equation dy/dx + 2y = e^(-x) with initial condition y(0) = 1.", None, "analytical_reasoning", 4),
    ("ar_ext_14", "Prove that in any group G, if x^2 = e for all x in G, then G is abelian.", None, "analytical_reasoning", 3),
    ("ar_ext_15", "Analyze the minimax regret strategy for decision making under uncertainty with state payoffs.", None, "analytical_reasoning", 4),

    # Writing
    ("wr_ext_01", "Write a thank you note to a mentor who gave career advice.", None, "writing", 1),
    ("wr_ext_02", "Draft an announcement email welcoming a new VP of Engineering to the company.", None, "writing", 2),
    ("wr_ext_03", "Compose a descriptive paragraph capturing the atmosphere of a bustling rainy market in Tokyo.", None, "writing", 2),
    ("wr_ext_04", "Write a persuasive proposal requesting approval to adopt Rust for our high-frequency trading engine.", None, "writing", 3),
    ("wr_ext_05", "Rewrite this technical explanation so that a 10-year-old child can understand how the internet works.", None, "writing", 2),
    ("wr_ext_06", "Draft an FAQ section addressing customer security concerns about cloud data migration.", None, "writing", 3),
    ("wr_ext_07", "Write a motivational speech for a startup team before launch day.", None, "writing", 2),
    ("wr_ext_08", "Compose a sonnet about the passage of time and autumn leaves.", None, "writing", 3),
    ("wr_ext_09", "Write an executive summary of our quarterly financial performance highlighting 25% ARR growth.", None, "writing", 3),
    ("wr_ext_10", "Draft a polite response to a negative customer review on our SaaS app store page.", None, "writing", 2),
    ("wr_ext_11", "Rewrite this passive sentence into active voice: 'The decision was made by the board after objections were raised.'", None, "writing", 1),
    ("wr_ext_12", "Compose a comprehensive RFP response for a government cloud infrastructure contract.", None, "writing", 5),
    ("wr_ext_13", "Write an engaging blog post introducing the new features of our open source library.", None, "writing", 3),
    ("wr_ext_14", "Draft a memo detailing the return-to-office hybrid policy updates.", None, "writing", 2),
    ("wr_ext_15", "Write a haiku about clean code.", None, "writing", 1),

    # Factual Lookup
    ("fl_ext_01", "What is the tallest mountain in North America?", None, "factual_lookup", 1),
    ("fl_ext_02", "Who was the Roman emperor during the Great Fire of Rome in 64 AD?", None, "factual_lookup", 1),
    ("fl_ext_03", "Define the term 'gerrymandering' in political science.", None, "factual_lookup", 1),
    ("fl_ext_04", "What is the boiling point of liquid nitrogen at 1 atm?", None, "factual_lookup", 1),
    ("fl_ext_05", "Who invented the World Wide Web and when was the first web page published?", None, "factual_lookup", 1),
    ("fl_ext_06", "What is the capital of Kazakhstan?", None, "factual_lookup", 1),
    ("fl_ext_07", "List the key differences between prokaryotic and eukaryotic cells.", None, "factual_lookup", 2),
    ("fl_ext_08", "When was the Magna Carta signed and by which king?", None, "factual_lookup", 1),
    ("fl_ext_09", "What is the function of the mitochondria in human cells?", None, "factual_lookup", 1),
    ("fl_ext_10", "What are the three laws of robotics formulated by Isaac Asimov?", None, "factual_lookup", 2),
    ("fl_ext_11", "Who painted 'The Starry Night' and where is it currently exhibited?", None, "factual_lookup", 1),
    ("fl_ext_12", "What is the half-life of Carbon-14 used in radiocarbon dating?", None, "factual_lookup", 2),
    ("fl_ext_13", "Which river is the longest in South America?", None, "factual_lookup", 1),
    ("fl_ext_14", "What is Moore's Law and who originated it?", None, "factual_lookup", 1),
    ("fl_ext_15", "Describe the historical context of the Meiji Restoration in 19th century Japan.", None, "factual_lookup", 3),

    # General
    ("gen_ext_01", "Could you suggest some low-maintenance houseplants for an apartment with indirect light?", None, "general", 1),
    ("gen_ext_02", "What should I pack for a 3-day camping trip in the mountains?", None, "general", 2),
    ("gen_ext_03", "Can you help me organize my daily schedule to fit in 30 minutes of exercise?", None, "general", 2),
    ("gen_ext_04", "What are some fun team-building activities for a remote engineering team?", None, "general", 2),
    ("gen_ext_05", "Thanks for your help! Have a great day.", None, "general", 1),
    ("gen_ext_06", "Can you tell me a clean joke about programming?", None, "general", 1),
    ("gen_ext_07", "What are some good strategies for overcoming writer's block?", None, "general", 2),
    ("gen_ext_08", "How can I improve my sleep hygiene?", None, "general", 2),
    ("gen_ext_09", "Can you help me choose between learning guitar or piano as an adult beginner?", None, "general", 2),
    ("gen_ext_10", "What are the rules of chess regarding castling?", None, "general", 2),
    ("gen_ext_11", "Could you give me some ideas for easy vegetarian dinners?", None, "general", 1),
    ("gen_ext_12", "What is the difference between a direct flight and a non-stop flight?", None, "general", 1),
    ("gen_ext_13", "How do noise-canceling headphones work?", None, "general", 2),
    ("gen_ext_14", "Can you give me a list of podcasts about technology and entrepreneurship?", None, "general", 2),
    ("gen_ext_15", "Just checking in, are you able to answer questions about finance?", None, "general", 1),
]

# Additional batch to ensure dataset reaches ~300 base groups:
def generate_additional_diverse_groups():
    groups = []
    classes = [
        ("code_generation", [
            ("Write a Lua script to parse Redis key expiration events.", 2),
            ("Implement an LRU cache in Swift using collections.", 3),
            ("Write a PowerShell script to audit active directory user account expirations.", 2),
            ("Build a parser for CSV files in Zig handling quoted commas.", 3),
            ("Implement a Bloom filter in Rust with optimal k hash functions.", 4),
            ("Write a Terraform module deploying an AWS VPC with private subnets and NAT gateways.", 3),
            ("Implement an AVL tree with rotation logic in Python.", 3),
            ("Write a C program to reverse a linked list iteratively and recursively.", 2),
            ("Create a custom React hook `useLocalStorage` with SSR hydration guards.", 2),
            ("Write an SQL query to calculate month-over-month retention cohort percentages.", 3),
            ("Implement an HTTP reverse proxy in Go with round-robin load balancing.", 4),
            ("Write a Solidity staking contract with time-weighted rewards.", 4),
            ("Implement an immutable persistent vector in Scala with structural sharing.", 5),
            ("Write a python script to convert markdown documentation into EPUB format.", 2),
            ("Implement a Trie data structure with prefix autocomplete in Kotlin.", 2),
            ("Write a CUDA kernel for 2D convolution with shared memory optimization.", 5),
            ("Implement the PageRank algorithm on a graph in PySpark.", 4),
            ("Write a WebGL fragment shader for procedural water wave reflections.", 4),
            ("Create a GitHub action that comments PR code test coverage diffs.", 2),
            ("Implement a distributed lock manager in Rust using etcd leases.", 4),
        ]),
        ("code_understanding", [
            ("Explain the concept of monads in functional programming with simple examples.", 3),
            ("What does the `std::move` cast actually do in C++?", 2),
            ("Explain why Python's default arguments evaluate at function definition time.", 1),
            ("How does Java's JIT compiler perform escape analysis and scalar replacement?", 4),
            ("Explain how TLS session resumption via tickets works.", 3),
            ("What is the difference between inner join, left join, and full outer join in SQL?", 1),
            ("Explain how Linux cgroups v2 control memory and CPU limits for containers.", 4),
            ("What is the ABA problem in lock-free concurrent programming and how is it solved?", 4),
            ("Explain what an async generator is in JavaScript and how `for await` consumes it.", 2),
            ("How does the V8 engine optimize JavaScript property access using hidden classes and inline caches?", 5),
            ("Explain the difference between TCP congestion control algorithms Cubic and BBR.", 4),
            ("What does `transmute` do in Rust and why is it inherently unsafe?", 2),
            ("Explain how zero-knowledge proofs work conceptually without deep algebra.", 3),
            ("What is tail latency amplification in distributed microservices?", 3),
            ("Explain how the Go runtime handles goroutine stack growth from 2KB.", 3),
            ("How does the Raft protocol prevent split-brain during network partitions?", 4),
            ("Explain the difference between row-oriented and columnar database storage engines.", 2),
            ("What is the purpose of the Linux `/proc` filesystem?", 2),
            ("Explain how deep learning backpropagation uses the multivariable chain rule.", 3),
            ("How does Git calculate SHA-1 object hashes for commits, trees, and blobs?", 3),
        ]),
        ("technical_design", [
            ("Design an order matching engine for a cryptocurrency exchange with microsecond latencies.", 5),
            ("Architect a real-time multiplayer game server backend using WebSockets and UDP.", 4),
            ("Design a distributed file upload system handling multi-gigabyte files with chunking and resume.", 3),
            ("What are the trade-offs between push-based and pull-based metrics monitoring systems?", 3),
            ("Design an access control system supporting Role-Based Access Control (RBAC) and Attribute-Based (ABAC).", 4),
            ("How should we design a feature flagging platform supporting percentage rollouts and targeting rules?", 3),
            ("Architect a serverless image processing pipeline triggered by S3 bucket events.", 2),
            ("Design a distributed tracing infrastructure collecting spans across 500 microservices.", 4),
            ("What are the architectural considerations when migrating from a monolith to microservices?", 3),
            ("Design a database schema for an online banking system with double-entry ledger.", 4),
            ("How should we design an API gateway for mobile apps to aggregate backend responses?", 3),
            ("Architect a globally replicated distributed key-value store using Paxos or Raft.", 5),
            ("Design a real-time bidding advertising exchange with 50ms strict SLA.", 5),
            ("What are the trade-offs of using Cassandra vs DynamoDB for time-series sensor telemetry?", 3),
            ("Design an incident management system with pager escalation and duty scheduling.", 3),
            ("Architect a secure enterprise secrets management service like HashiCorp Vault.", 4),
            ("Design a web crawling system that respects robots.txt and crawls 100 million pages daily.", 4),
            ("How should we design a multi-region disaster recovery strategy with RPO=0 and RTO<1m?", 5),
            ("Design a scalable commenting system supporting nested replies and upvotes.", 2),
            ("Architect a vector database search system for 1 billion 768-dimensional embeddings.", 5),
        ]),
        ("analytical_reasoning", [
            ("Calculate the probability of drawing a full house in 5-card poker.", 3),
            ("Solve the Tower of Hanoi problem for 4 disks and calculate minimum moves.", 2),
            ("Prove that the language L = {a^n b^n c^n | n >= 0} is not context-free using the pumping lemma.", 4),
            ("Calculate the trajectory and range of a projectile launched at 45 degrees with initial velocity 20 m/s.", 2),
            ("Solve this logic puzzle: who owns the zebra among 5 houses of different colors and pets?", 3),
            ("Calculate the limit as x approaches 0 of (sin(x) - x) / x^3 using L'Hopital's rule.", 2),
            ("Prove that every tree with n vertices has exactly n - 1 edges.", 3),
            ("Calculate the Gini coefficient for a population with income distribution [10, 20, 30, 40].", 3),
            ("Solve for x: log2(x) + log2(x - 2) = 3.", 2),
            ("Calculate the conditional probability P(Disease | Positive Test) given 99% accuracy and 0.1% base rate.", 3),
            ("Prove that the set of real numbers is uncountable using Cantor's diagonal argument.", 4),
            ("Calculate the resistance between opposite corners of a cube made of 1-ohm resistors.", 4),
            ("Derive the formula for compound interest continuously compounded.", 2),
            ("Calculate the stationary distribution of a 3-state Markov chain.", 3),
            ("Solve the 8-queens problem and determine how many distinct non-isomorphic solutions exist.", 3),
            ("Calculate the volume integral of x^2 + y^2 over a cylinder of radius R and height H.", 3),
            ("Prove that the square of any odd integer is of the form 8k + 1.", 2),
            ("Calculate the Shannon entropy of a source with symbol probabilities [0.5, 0.25, 0.125, 0.125].", 2),
            ("Analyze the game of Nim with pile sizes (3, 4, 5) and determine the winning first move.", 3),
            ("Derive the Euler-Lagrange equation for a simple harmonic pendulum.", 4),
            ("Derive the closed-form Black-Scholes formula using risk-neutral pricing.", 5),
            ("Derive the optimal policy for a Markov Decision Process with state transition probabilities.", 5),
            ("Calculate the Fourier transform of a Gaussian function f(x) = exp(-a*x^2).", 4),
            ("Prove that every subgroup of an abelian group is normal.", 3),
            ("Solve this linear recurrence: a_n = 5*a_{n-1} - 6*a_{n-2} with a_0=1, a_1=4.", 3),
            ("Calculate the expected value and variance of a Poisson distributed variable.", 2),
            ("Derive the backpropagation gradient equations for a 2-layer neural network.", 4),
            ("Prove by mathematical induction that the sum of first n squares is n(n+1)(2n+1)/6.", 3),
            ("Calculate the eigenvalues and eigenvectors of a symmetric 3x3 covariance matrix.", 3),
            ("Solve the traveling salesperson problem for a 5-node graph using dynamic programming.", 4),
        ]),
        ("writing", [
            ("Write an email requesting feedback from your team after completing a major project milestone.", 2),
            ("Draft a customer service response resolving a shipping delay complaint gracefully.", 2),
            ("Compose an introductory paragraph for an article about the future of renewable energy.", 2),
            ("Write a script for a podcast episode introducing quantum computing concepts.", 3),
            ("Draft an internal memo announcing a scheduled server maintenance window.", 1),
            ("Compose a heartfelt retirement speech honoring a colleague with 30 years of service.", 3),
            ("Write a product announcement newsletter showcasing our new mobile app.", 2),
            ("Draft guidelines for contributing to an open source project.", 2),
            ("Rewrite this email to make it more empathetic and understanding.", 1),
            ("Compose a fantasy story opening describing an ancient ruined citadel at dusk.", 3),
            ("Write a recommendation letter for an engineering intern applying to graduate school.", 3),
            ("Draft an apology letter to customers affected by an unexpected billing glitch.", 2),
            ("Compose a punchy elevator pitch for a B2B AI analytics startup.", 2),
            ("Write a short blog post on five practical habits for writing cleaner code.", 2),
            ("Draft an executive summary for a board presentation on cybersecurity risks.", 4),
            ("Compose a poem celebrating the beauty of mathematics.", 2),
            ("Write a press release announcing a partnership between two healthcare companies.", 3),
            ("Draft a polite follow-up email after a job interview.", 1),
            ("Rewrite this paragraph to eliminate jargon and improve readability.", 1),
            ("Compose a comprehensive policy proposal on remote work sustainability.", 4),
        ]),
        ("factual_lookup", [
            ("What is the capital of Canada?", 1),
            ("Who was the 16th President of the United States?", 1),
            ("What is the chemical formula for photosynthesis?", 1),
            ("Define the term 'heuristic' in computer science.", 1),
            ("When was the United Nations founded?", 1),
            ("What is the deepest trench in the world's oceans?", 1),
            ("Who discovered the structure of DNA in 1953?", 1),
            ("What are the primary colors in additive color mixing (RGB)?", 1),
            ("What is the currency of Japan?", 1),
            ("Which planet in our solar system has the most moons?", 1),
            ("Define the term 'quantum entanglement'.", 2),
            ("When did the Apollo 11 mission land on the Moon?", 1),
            ("What is the boiling point of ethanol in Celsius?", 1),
            ("Who wrote the novel 'Crime and Punishment'?", 1),
            ("What is the main function of the kidneys in the human body?", 1),
            ("List the seven continents in order of land area.", 2),
            ("What is Avogadro's constant and what does it represent?", 2),
            ("When was the fall of the Berlin Wall?", 1),
            ("What is the SI unit of electrical resistance?", 1),
            ("Define the economic concept of 'opportunity cost'.", 1),
        ]),
        ("general", [
            ("Can you recommend three great science fiction books to read?", 2),
            ("How can I organize my desk to be more productive?", 1),
            ("What are some easy hobbies to pick up on weekends?", 1),
            ("How do I care for a sourdough bread starter?", 2),
            ("What are some conversational icebreakers for a networking event?", 1),
            ("Can you help me plan a surprise birthday party for my friend?", 2),
            ("What are the health benefits of drinking green tea?", 1),
            ("How can I prepare my car for winter weather driving?", 2),
            ("Can you suggest some budget-friendly weekend road trip destinations in New England?", 2),
            ("What is the etiquette for tipping in restaurants in European countries?", 1),
            ("Can you help me brainstorm a title for my podcast about personal finance?", 2),
            ("What are some tips for staying calm before public speaking?", 2),
            ("How do air fryers work compared to conventional convection ovens?", 2),
            ("Can you suggest some relaxing music playlists for studying?", 1),
            ("What are some fun games to play with family during holidays?", 1),
            ("Could you give me some tips for starting a backyard vegetable garden?", 2),
            ("How do I choose the right running shoes for my foot shape?", 2),
            ("Can you summarize the difference between espresso and drip coffee?", 1),
            ("What are some good podcasts to listen to on long commutes?", 1),
            ("Hello assistant, can you tell me what you can help me with today?", 1),
        ]),
    ]
    idx = 1
    for rt, items in classes:
        for q, comp in items:
            groups.append({
                "group": f"auto_{rt}_{idx:02d}",
                "query": q,
                "context": None,
                "request_type": rt,
                "complexity": comp,
                "derived": []
            })
            idx += 1
    return groups

def build_full_dataset():
    random.seed(42)
    groups = []

    # Add RAW_DATA
    for item in RAW_DATA:
        derived_list = item.get("derived", [])
        groups.append({
            "group": item["group"],
            "query": item["query"],
            "context": item.get("context"),
            "request_type": item["request_type"],
            "complexity": item["complexity"],
            "derived": derived_list,
        })

    # Add ADDITIONAL_DATA
    for gid, q, ctx, rt, comp in ADDITIONAL_DATA:
        groups.append({
            "group": gid,
            "query": q,
            "context": ctx,
            "request_type": rt,
            "complexity": comp,
            "derived": [],
        })

    # Add auto-generated classes
    groups.extend(generate_additional_diverse_groups())

    print(f"Total base source groups: {len(groups)}")

    # Add automatic derivations to reach ~130 derived examples:
    # 1. Typos
    # 2. Padded wrappers
    # 3. Paraphrases
    derived_count = 0
    for g in groups:
        if derived_count >= 140:
            break
        if not g["derived"]:
            # Pick candidates for perturbation
            q = g["query"]
            rt = g["request_type"]
            comp = g["complexity"]

            # Padded wrapper
            padded = f"Hello, I have a quick question. {q} Looking forward to your response, thanks!"
            g["derived"].append((padded, g["context"], comp))
            derived_count += 1

            if derived_count < 140 and len(q.split()) > 5:
                # Slight paraphrase / informal
                informal = "Can you " + q[0].lower() + q[1:]
                if not informal.endswith("?") and not informal.endswith("."):
                    informal += "?"
                g["derived"].append((informal, g["context"], comp))
                derived_count += 1

    # Stratified group splitting: partition groups by request_type
    groups_by_class = {}
    for g in groups:
        groups_by_class.setdefault(g["request_type"], []).append(g)

    train_groups = set()
    calib_groups = set()
    val_groups = set()

    for rt, g_list in groups_by_class.items():
        random.shuffle(g_list)
        n = len(g_list)
        n_train = int(n * 0.70)
        n_calib = int(n * 0.15)
        for g in g_list[:n_train]:
            train_groups.add(g["group"])
        for g in g_list[n_train : n_train + n_calib]:
            calib_groups.add(g["group"])
        for g in g_list[n_train + n_calib :]:
            val_groups.add(g["group"])

    # Assign split to each group
    dataset = []
    item_id = 1
    for g in groups:
        gid = g["group"]
        if gid in train_groups:
            split = "train"
        elif gid in calib_groups:
            split = "calibration"
        else:
            split = "validation"

        # Base example
        dataset.append({
            "id": f"ex_{item_id:04d}",
            "source_group": gid,
            "query": g["query"],
            "context": g["context"],
            "request_type": g["request_type"],
            "complexity": g["complexity"],
            "split": split,
            "is_derived": False,
        })
        item_id += 1

        # Derived examples
        for dq, dctx, dcomp in g["derived"]:
            dataset.append({
                "id": f"ex_{item_id:04d}",
                "source_group": gid,
                "query": dq,
                "context": dctx,
                "request_type": g["request_type"], # label preserves type
                "complexity": dcomp,
                "split": split,
                "is_derived": True,
            })
            item_id += 1

    print(f"Total dataset examples: {len(dataset)}")
    train_count = sum(1 for d in dataset if d["split"] == "train")
    calib_count = sum(1 for d in dataset if d["split"] == "calibration")
    val_count = sum(1 for d in dataset if d["split"] == "validation")
    print(f"Splits: train={train_count}, calibration={calib_count}, validation={val_count}")

    # Check class distributions
    for sp in ["train", "calibration", "validation"]:
        counts = {}
        for d in dataset:
            if d["split"] == sp:
                counts[d["request_type"]] = counts.get(d["request_type"], 0) + 1
        print(f"Split '{sp}' request_type distribution: {counts}")

    # Save to JSON
    with open("llm-router/training/dataset.json", "w", encoding="utf-8") as f:
        json.dump({"dataset": dataset}, f, indent=2)

    # Also save evaluation split in the official EVAL_SET format for classifier_eval example
    val_examples = [
        {
            "id": d["id"],
            "query": d["query"],
            "context": d["context"],
            "ground_truth_type": d["request_type"],
            "ground_truth_complexity": d["complexity"],
        }
        for d in dataset if d["split"] == "validation"
    ]
    with open("llm-router/training/eval_val_set.json", "w", encoding="utf-8") as f:
        json.dump({"examples": val_examples}, f, indent=2)

    print("Saved dataset.json and eval_val_set.json successfully.")

if __name__ == "__main__":
    build_full_dataset()
