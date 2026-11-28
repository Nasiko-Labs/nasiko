import base64
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import runner
from example import x11_mcp

HERE = Path(__file__).resolve().parent
EXAMPLE = HERE / "example"


class RunnerTest(unittest.TestCase):
    def test_offline_report_has_one_distinct_router_tag_per_slot(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            output = root / "results"
            succeed = [sys.executable, "-c", "pass"]
            fixture = {
                "version": 1, "tasks": [{
                    "id": "confirm-ticket", "source": "fixture", "prompt": "Confirm the ticket",
                    "reset": succeed,
                    "grade": [sys.executable, "-c", "import json; print(json.dumps(dict(resolved=True)))"],
                }],
                "harnesses": {"scripted": succeed}, "models": ["model-a", "model-b"], "trials": 2,
            }
            manifest = root / "tasks.json"
            manifest.write_text(json.dumps(fixture))
            subprocess.run([sys.executable, str(HERE / "runner.py"), str(manifest), "--output", str(output)],
                           check=True, capture_output=True)
            rows = json.loads((output / "report.json").read_text())["attempts"]
            self.assertEqual({(r["model"], r["trial"]) for r in rows},
                             {("model-a", 1), ("model-a", 2), ("model-b", 1), ("model-b", 2)})
            self.assertEqual(len({r["router_tag"] for r in rows}), 4)
            self.assertTrue(all(r["resolved"] is True and r["usage_status"] == "offline" for r in rows))
            self.assertTrue(all(r["router_usd"] is None for r in rows))

    def test_saved_work_is_graded_after_a_client_error(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            def command(source):
                return [sys.executable, "-c", source]

            artifact = "from pathlib import Path; import os; saved = Path(os.environ['DESKTOP_ATTEMPT_DIR'])/'saved';"
            task = {"id": "confirm", "prompt": "Save", "reset": command("pass"),
                    "grade": command(artifact + "import json; print(json.dumps({'resolved': saved.is_file()}))")}
            row = runner.attempt(root, root, "test", 1, task, "client",
                                 command(artifact + "saved.touch(); raise SystemExit(1)"),
                                 "model", 1, 5, None)
            self.assertIs(row["resolved"], True)
            self.assertEqual(row["harness_outcome"], "harness_error")
            self.assertEqual(row["outcome"], "harness_error")

    def test_client_json_errors_write_only_safe_diagnostics(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for harness in ("codex", "opencode"):
                with self.subTest(harness=harness):
                    directory = root / (harness + "-attempt")
                    directory.mkdir()
                    message = "Rate limit exceeded. Bearer private-fixture; body contains private prompt."
                    event = ({"type": "turn.failed", "error": {"message": message + " HTTP 429"}}
                             if harness == "codex" else
                             {"type": "error", "error": {"data": {"statusCode": 429, "message": message}}})
                    cli = root / harness
                    cli.write_text(f"#!{sys.executable}\nprint({json.dumps(event)!r})\n")
                    cli.chmod(0o755)
                    env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                               DESKTOP_PROMPT="Save", DESKTOP_MODEL="test", DESKTOP_HARNESS=harness,
                               DESKTOP_ATTEMPT_DIR=str(directory), NASIKO_DESKTOP_MCP=str(cli),
                               NASIKO_ROUTER_URL="http://localhost/v1", NASIKO_ROUTER_TOKEN="private-fixture",
                               NASIKO_EVAL_ATTEMPT="test:trial")
                    result = subprocess.run([sys.executable, str(HERE / "adapters/desktop_harness.py")], env=env)
                    self.assertEqual(result.returncode, 1)
                    diagnostic = json.loads((directory / "harness-errors.jsonl").read_text())
                    self.assertEqual(diagnostic, {"event": event["type"], "reason": "rate limited", "http_status": 429})

    def test_timeout_grades_the_saved_artifact_without_passing_control_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            def command(code):
                return [sys.executable, "-c", code]
            check_env = "import os;assert 'NASIKO_API_TOKEN' not in os.environ;"
            no_token = "assert 'NASIKO_ROUTER_TOKEN' not in os.environ;"
            task = {"id":"ticket", "prompt":"Save ticket", "reset":command(check_env + no_token),
                    "grade":command(check_env + no_token +
                        "from pathlib import Path;import json;"
                        "print(json.dumps({'resolved':(Path(os.environ['DESKTOP_ATTEMPT_DIR'])/'saved').exists()}))")}
            harness = command(check_env + "assert os.environ['NASIKO_ROUTER_TOKEN']=='fixture';"
                              "from pathlib import Path;import time;"
                              "(Path(os.environ['DESKTOP_ATTEMPT_DIR'])/'saved').touch();time.sleep(20)")
            with patch.dict(os.environ, NASIKO_API_TOKEN="controller", NASIKO_ROUTER_TOKEN="fixture"):
                result = runner.attempt(root, root, "run", 1, task, "h", harness, "m", 1, 1, "http://localhost:8080")
            self.assertEqual(result["outcome"], "time_limit")
            self.assertIs(result["resolved"], True)
            self.assertEqual(result["router_tag"], "run:a00001")

    def test_router_evidence_preserves_missing_mismatch_fallback_and_true_zero(self):
        attempts = []
        rows = {}
        for tag, models, source in (
                ("missing", [], None), ("mismatch", ["other"], "Exact"),
                ("fallback", ["m"], "Default"), ("zero", ["m"], "Exact")):
            attempts.append({"router_tag":tag, "model":"m", "router_usage":None})
            if source:
                rows[tag] = {"models":models, "cost_usd":0.0, "worst_price_source":source}
        with patch.dict(os.environ, NASIKO_API_TOKEN="fixture"), patch.object(runner, "collect_usage", return_value=rows):
            runner.attach_usage({"attempts":attempts, "run_id":"run"}, "http://localhost")
        self.assertEqual([r["usage_status"] for r in attempts], ["unavailable","model_mismatch","unpriced","recorded"])
        self.assertEqual([r["router_usd"] for r in attempts], [None,None,None,0.0])

    def test_screenshot_evidence_matches_the_model_observation(self):
        png = base64.b64decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8"
            "/x8AAwMCAO+tWZkAAAAASUVORK5CYII="
        )
        with tempfile.TemporaryDirectory() as temp:
            with patch.dict(os.environ, DESKTOP_ATTEMPT_DIR=temp), patch.object(
                x11_mcp.subprocess, "check_output", return_value=png
            ):
                response = x11_mcp.result("tools/call", {"name": "observe", "arguments": {}})
            frames = list((Path(temp) / "screenshots").glob("*.png"))
            self.assertEqual(len(frames), 1)
            self.assertEqual(base64.b64decode(response["content"][0]["data"]), frames[0].read_bytes())

    def test_grader_distinguishes_missing_wrong_and_saved(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory/"ticket.txt").write_text("OPS-123")
            def grade():
                result = subprocess.check_output([sys.executable, str(EXAMPLE/"ticket.py"), "grade"],
                                                 env={**os.environ, "DESKTOP_ATTEMPT_DIR":tmp})
                return json.loads(result)["resolved"]
            self.assertIs(grade(), False)
            self.assertFalse((directory/"submitted.json").exists())
            (directory/"submitted.json").write_text('{"ticket":"OPS-999"}')
            self.assertIs(grade(), False)
            (directory/"submitted.json").write_text('{"ticket":"OPS-123"}')
            self.assertIs(grade(), True)

    @unittest.skipUnless(all(shutil.which(name) for name in ("xvfb-run", "xdotool", "import")),
                         "Linux X11 example requires xvfb-run, xdotool and ImageMagick")
    def test_scripted_gui(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)/"run"
            subprocess.run(["xvfb-run","-a",sys.executable,str(HERE/"runner.py"),str(EXAMPLE/"task.json"),
                            "--output",str(out)],check=True,timeout=40)
            row = json.loads((out/"report.json").read_text())["attempts"][0]
            self.assertEqual((row["outcome"],row["resolved"]),("finished",True))
            self.assertEqual(len(row["screenshots"]),2)
            self.assertIsNone(row["router_usd"])


if __name__ == "__main__":
    unittest.main()
