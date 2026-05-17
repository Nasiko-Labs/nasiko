import logging
import asyncio
import unittest


from app.service.observability_service import ObservabilityService


class AgentMetricsService(ObservabilityService):
    def __init__(self):
        super().__init__(logging.getLogger("test-agent-metrics"))
        self.registries = {
            "translator": {
                "id": "translator",
                "name": "Translator",
                "description": "Translates text",
            },
            "summarizer": {
                "id": "summarizer",
                "name": "Summarizer",
                "description": "Summarizes text",
            },
        }
        self.stats = {
            "translator": {
                "data": {
                    "project": {
                        "trace_count": 3,
                        "latency_ms_p50": 1200,
                        "latency_ms_p99": 2500,
                        "streaming_last_updated_at": "2026-05-17T01:45:00.000Z",
                    }
                }
            },
            "summarizer": RuntimeError("Phoenix is unavailable"),
        }
        self.sessions = {
            "translator": [
                {
                    "session_id": "session-a",
                    "num_traces": 2,
                    "start_time": "2026-05-17T00:15:00.000Z",
                    "trace_latency_ms_p50": 1000,
                    "trace_latency_ms_p99": 2100,
                    "traces": [
                        {
                            "trace_id": "trace-a",
                            "root_span": {
                                "latency_ms": 900,
                                "start_time": "2026-05-17T00:15:00.000Z",
                                "status_code": "OK",
                            },
                        },
                        {
                            "trace_id": "trace-b",
                            "root_span": {
                                "latency_ms": 1300,
                                "start_time": "2026-05-17T00:45:00.000Z",
                                "status_code": "ERROR",
                            },
                        },
                    ],
                },
                {
                    "session_id": "session-b",
                    "num_traces": 1,
                    "start_time": "2026-05-17T01:15:00.000Z",
                    "trace_latency_ms_p50": 1200,
                    "traces": [
                        {
                            "trace_id": "trace-c",
                            "root_span": {
                                "latency_ms": 1400,
                                "start_time": "2026-05-17T01:15:00.000Z",
                                "status_code": "UNSET",
                            },
                        }
                    ],
                },
            ],
            "summarizer": [],
        }

    async def _get_agent_registry_summaries(self, agent_ids):
        return [
            self.registries[agent_id]
            for agent_id in agent_ids
            if agent_id in self.registries
        ]

    async def _get_accessible_agent_ids(self, auth_header):
        return ["translator", "summarizer"]

    async def get_agent_project_stats(self, agent_id, start_time):
        response = self.stats.get(agent_id)
        if isinstance(response, Exception):
            raise response
        return response

    async def _get_project_id(self, project_name):
        return f"project-{project_name}"

    async def _get_project_sessions_for_aggregation(
        self, project_id, agent_id, start_time=None
    ):
        return self.sessions.get(agent_id, [])


class AgentMetricsTestCase(unittest.TestCase):
    def test_agent_metrics_contract_aggregates_registry_and_phoenix_data(self):
        service = AgentMetricsService()

        result = asyncio.run(
            service.get_agent_performance_metrics(
                user_id="user-1",
                auth_header="Bearer token",
                window_hours=24,
                now_iso="2026-05-17T02:00:00.000Z",
            )
        )

        self.assertEqual(result["data"]["window"]["hours"], 24)
        self.assertEqual(result["data"]["summary"]["total_agents"], 2)
        self.assertEqual(result["data"]["summary"]["active_agents"], 1)
        self.assertEqual(result["data"]["summary"]["total_requests"], 3)
        self.assertEqual(result["data"]["summary"]["success_count"], 2)
        self.assertEqual(result["data"]["summary"]["error_count"], 1)
        self.assertAlmostEqual(result["data"]["summary"]["error_rate"], 33.33)
        self.assertAlmostEqual(result["data"]["summary"]["average_latency_ms"], 1200.0)

        translator = result["data"]["agents"][0]
        self.assertEqual(translator["agent_id"], "translator")
        self.assertEqual(translator["agent_name"], "Translator")
        self.assertEqual(translator["status"], "active")
        self.assertEqual(translator["success_count"], 2)
        self.assertEqual(translator["error_count"], 1)
        self.assertAlmostEqual(translator["uptime_percentage"], 66.67)
        self.assertAlmostEqual(translator["average_latency_ms"], 1200.0)
        self.assertEqual(translator["p50_latency_ms"], 1200)
        self.assertEqual(translator["p99_latency_ms"], 2500)
        self.assertEqual(translator["last_activity_at"], "2026-05-17T01:45:00.000Z")
        self.assertEqual(len(translator["hourly"]), 24)
        self.assertEqual(translator["hourly"][-3]["requests"], 2)
        self.assertEqual(translator["hourly"][-2]["requests"], 1)
        self.assertEqual(translator["hourly"][-1]["requests"], 0)

    def test_agent_metrics_keep_zero_rows_when_phoenix_agent_fails(self):
        service = AgentMetricsService()

        result = asyncio.run(
            service.get_agent_performance_metrics(
                user_id="user-1",
                auth_header="Bearer token",
                window_hours=24,
                now_iso="2026-05-17T02:00:00.000Z",
            )
        )

        summarizer = result["data"]["agents"][1]
        self.assertEqual(summarizer["agent_id"], "summarizer")
        self.assertEqual(summarizer["status"], "unavailable")
        self.assertEqual(summarizer["requests"], 0)
        self.assertEqual(summarizer["success_count"], 0)
        self.assertEqual(summarizer["error_count"], 0)
        self.assertEqual(summarizer["uptime_percentage"], 0.0)
        self.assertEqual(summarizer["error"], "Phoenix is unavailable")

    def test_hourly_buckets_keep_traces_from_first_partial_hour(self):
        service = AgentMetricsService()
        start = service._parse_iso_datetime("2026-05-17T13:45:00.000Z")

        buckets = service._build_hourly_buckets(
            [
                {
                    "latency_ms": 800,
                    "start_time": "2026-05-17T13:50:00.000Z",
                    "status_code": "OK",
                }
            ],
            start,
            2,
        )

        self.assertEqual(buckets[0]["time"], "2026-05-17T13:00:00.000Z")
        self.assertEqual(buckets[0]["requests"], 1)
        self.assertEqual(buckets[0]["success_count"], 1)
        self.assertEqual(buckets[0]["average_latency_ms"], 800.0)


if __name__ == "__main__":
    unittest.main()
