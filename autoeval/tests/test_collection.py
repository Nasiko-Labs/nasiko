"""Host collector contracts: missing, rewritten, or inconsistent output fails closed."""

import json
import sys
import unittest

from nasiko_autoeval.capture import capture
from nasiko_autoeval.results import parse_results
from nasiko_autoeval.schema import EvalError


def stream(events, token="host-token"):
    return "\n".join(
        "\x1eNASIKO_PYTEST_V1 " + token + " " + json.dumps(event) for event in events
    )


def events(outcome="passed", code=0):
    return [
        {"event": "start"},
        *[
            {
                "event": "test",
                "id": "tests/test_add.py::test_add",
                "phase": phase,
                "outcome": outcome if phase == "call" else "passed",
            }
            for phase in ("setup", "call", "teardown")
        ],
        {"event": "finish", "exit_code": code},
    ]


class CollectorTests(unittest.TestCase):
    def parse(self, output, code=0):
        return parse_results(
            "pytest-events", None, output, protocol_token="host-token", exit_code=code
        )

    def test_complete_stream_preserves_per_test_status(self):
        self.assertEqual(
            self.parse(stream(events())), {"tests.test_add::test_add": "passed"}
        )
        self.assertEqual(
            self.parse(stream(events("failed", 1)), 1),
            {"tests.test_add::test_add": "failed"},
        )

    def test_late_forced_zero_exit_cannot_erase_reported_failure(self):
        with self.assertRaisesRegex(EvalError, "exit status"):
            self.parse(stream(events("failed", 1)), 0)

    def test_printed_junit_cannot_replace_observed_test_events(self):
        fake = '<testsuite><testcase classname="tests.test_add" name="test_add"/></testsuite>'
        self.assertEqual(
            self.parse(stream(events("failed", 1)) + "\n" + fake, 1)[
                "tests.test_add::test_add"
            ],
            "failed",
        )
        with self.assertRaisesRegex(EvalError, "start/finish"):
            self.parse(fake)

    def test_missing_footer_zero_tests_duplicate_phases_and_wrong_token_rejected(self):
        broken = [
            events()[:-1],
            [events()[0], events()[-1]],
            events()[:2] + events()[1:],
        ]
        for records in broken:
            if len(records) == 2:
                # Zero collection is rejected by normalize, not this phase decoder.
                self.assertEqual(self.parse(stream(records)), {})
            else:
                with self.subTest(records=records), self.assertRaises(EvalError):
                    self.parse(stream(records))
        with self.assertRaisesRegex(EvalError, "token"):
            self.parse(stream(events(), token="wrong-run"))

    def test_extra_finish_and_out_of_order_call_are_rejected(self):
        for records in (
            events() + [events()[-1]],
            [events()[0], events()[2], events()[1], events()[3], events()[-1]],
        ):
            with self.assertRaises(EvalError):
                self.parse(stream(records))

    def test_capture_keeps_streams_separate_without_container_files(self):
        result = capture(
            [sys.executable, "-c", "import os; os.write(1,b'out'); os.write(2,b'err')"],
            5,
        )
        self.assertEqual(
            (result.stdout, result.stderr, result.stopped), (b"out", b"err", None)
        )

    def test_output_limit_and_timeout_stop_the_capture(self):
        result = capture(
            [sys.executable, "-c", "import os; os.write(1,b'x'*10000)"],
            5,
            max_bytes=100,
        )
        self.assertEqual(result.stopped, "output_limit")
        self.assertEqual(len(result.stdout) + len(result.stderr), 100)
        result = capture([sys.executable, "-c", "import time; time.sleep(10)"], 0.05)
        self.assertEqual(result.stopped, "timeout")


if __name__ == "__main__":
    unittest.main()
