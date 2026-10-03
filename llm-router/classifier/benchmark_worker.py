"""Development probe. Official evaluation remains the Rust classifier_eval example."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import time

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--model-path", type=Path, required=True)
    p.add_argument("--dataset", type=Path, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--threads", type=int, default=2)
    args = p.parse_args()
    root = Path(__file__).resolve().parent
    questions = json.loads((root.parent / "assets/classifier_questions.json").read_text())
    cases = json.loads(args.dataset.read_text(encoding="utf-8"))["examples"]
    started = time.perf_counter()
    proc = subprocess.Popen([sys.executable, str(root / "model_worker.py"), "--model-path", str(args.model_path), "--threads", str(args.threads)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    try:
        ready = json.loads(proc.stdout.readline())
        assert ready.get("ready"), ready
        print(json.dumps({"load_ms": round((time.perf_counter() - started) * 1000)}), flush=True)
        with args.output.open("w", encoding="utf-8") as out:
            for case in cases:
                payload = {"state": {"query": case["query"], "context": case.get("context") or ""}, "questions": questions}
                started = time.perf_counter_ns()
                proc.stdin.write(json.dumps(payload) + "\n")
                proc.stdin.flush()
                raw = json.loads(proc.stdout.readline())
                latency = (time.perf_counter_ns() - started) // 1000
                if "answers" not in raw:
                    raise RuntimeError("worker inference failed")
                types = raw["answers"]["request_type"]["probabilities"]
                complexity = raw["answers"]["complexity"]["probabilities"]
                winner = max(types, key=types.get)
                row = {"id": case["id"], "request_type": winner, "complexity": int(max(complexity, key=complexity.get)) + 1, "confidence": types[winner], "latency_us": latency}
                out.write(json.dumps(row) + "\n")
                out.flush()
                print(json.dumps(row), flush=True)
    finally:
        proc.stdin.close()
        proc.wait(timeout=30)

if __name__ == "__main__":
    main()
