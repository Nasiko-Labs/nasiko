import logging
import unittest

from app.utils.log_buffer import (
    PlatformLogBufferHandler,
    clear_platform_log_buffer,
    get_recent_platform_logs,
)


class PlatformLogBufferHandlerTests(unittest.TestCase):
    def setUp(self):
        clear_platform_log_buffer()
        self.logger = logging.getLogger("app.tests.platform_logs")
        self.handler = PlatformLogBufferHandler()
        self.original_level = self.logger.level
        self.original_propagate = self.logger.propagate
        self.logger.setLevel(logging.INFO)
        self.logger.propagate = False
        self.logger.addHandler(self.handler)

    def tearDown(self):
        self.logger.removeHandler(self.handler)
        self.logger.setLevel(self.original_level)
        self.logger.propagate = self.original_propagate
        clear_platform_log_buffer()

    def test_captures_structured_request_metadata(self):
        self.logger.info(
            "GET /api/v1/agents -> 200",
            extra={
                "service": "nasiko-backend",
                "route": "GET /api/v1/agents",
                "trace_id": "trace-123",
                "request_id": "req-123",
                "latency_ms": 42,
                "status_code": 200,
                "pod": "backend-test",
                "commit": "abc123",
                "source": "test",
            },
        )

        [entry] = get_recent_platform_logs()

        self.assertEqual(entry["level"], "INFO")
        self.assertEqual(entry["service"], "nasiko-backend")
        self.assertEqual(entry["route"], "GET /api/v1/agents")
        self.assertEqual(entry["trace_id"], "trace-123")
        self.assertEqual(entry["request_id"], "req-123")
        self.assertEqual(entry["latency_ms"], 42)
        self.assertEqual(entry["status_code"], 200)
        self.assertEqual(entry["pod"], "backend-test")
        self.assertEqual(entry["commit"], "abc123")
        self.assertEqual(entry["source"], "test")

    def test_filters_recent_logs_by_level(self):
        self.logger.info("startup complete")
        self.logger.warning("latency budget exceeded")

        warning_logs = get_recent_platform_logs(level="WARNING")

        self.assertEqual(len(warning_logs), 1)
        self.assertEqual(warning_logs[0]["message"], "latency budget exceeded")


if __name__ == "__main__":
    unittest.main()
