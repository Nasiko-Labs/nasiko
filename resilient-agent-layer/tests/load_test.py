"""
Locust load test for the Resilient Agent Request Layer.

Run:
    locust -f tests/load_test.py --headless -u 50 -r 5 --run-time 60s --host http://localhost:8000

Scenarios:
1. Repeated requests → should show high cache hit rate
2. Burst to slow agent → queue should absorb, low 429 rate
3. Novel requests → measures cold path latency
"""
from locust import HttpUser, task, between
import random
import json

AGENTS = ["agent-a", "agent-b", "agent-slow"]
FIXED_QUERIES = [f"query-{i}" for i in range(10)]  # repeated = cache hits
NOVEL_QUERIES = [f"novel-{random.random()}" for _ in range(1000)]


class AgentGatewayUser(HttpUser):
    wait_time = between(0.1, 0.5)

    @task(5)
    def repeated_request(self):
        """High probability of cache hit."""
        payload = {
            "agent_id": random.choice(["agent-a", "agent-b"]),
            "payload": {"query": random.choice(FIXED_QUERIES)},
        }
        with self.client.post("/invoke", json=payload, catch_response=True) as resp:
            if resp.status_code == 200:
                data = resp.json()
                if data.get("source") == "cache":
                    resp.success()
            elif resp.status_code == 429:
                resp.success()  # Expected when rate limited
            else:
                resp.failure(f"Unexpected {resp.status_code}")

    @task(2)
    def novel_request(self):
        """Unique queries — always cache miss, tests cold path."""
        payload = {
            "agent_id": "agent-a",
            "payload": {"query": random.choice(NOVEL_QUERIES)},
        }
        with self.client.post("/invoke", json=payload, catch_response=True) as resp:
            if resp.status_code in (200, 429, 503):
                resp.success()
            else:
                resp.failure(f"Unexpected {resp.status_code}")

    @task(1)
    def burst_slow_agent(self):
        """Targets slow agent to test queue absorption."""
        payload = {
            "agent_id": "agent-slow",
            "payload": {"query": random.choice(FIXED_QUERIES)},
            "priority": random.randint(0, 5),
        }
        with self.client.post("/invoke", json=payload, catch_response=True, timeout=20) as resp:
            if resp.status_code in (200, 429, 503):
                resp.success()
            else:
                resp.failure(f"Unexpected {resp.status_code}")

    @task(1)
    def check_health(self):
        self.client.get("/health")

    @task(1)
    def check_stats(self):
        self.client.get("/admin/stats")
