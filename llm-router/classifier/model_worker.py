"""Persistent offline model transport. Only protocol data goes to stdout."""
import argparse
import contextlib
import json
import os
from pathlib import Path
import sys

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-path", type=Path, required=True)
    parser.add_argument("--threads", type=int, default=2)
    args = parser.parse_args()
    if not args.model_path.is_dir() or args.threads < 1:
        parser.error("an existing local model directory and positive thread count are required")
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["USE_TF"] = "0"
    with contextlib.redirect_stdout(sys.stderr):
        if (args.model_path / "semantic_model.json").is_file():
            from semantic_inference import SemanticModel
            agent = SemanticModel(args.model_path, args.threads)
            inference = contextlib.nullcontext
        else:
            import torch
            import laya
            torch.set_num_threads(args.threads)
            torch.set_num_interop_threads(1)
            torch.manual_seed(42)
            torch.use_deterministic_algorithms(True)
            agent = laya.load(str(args.model_path.resolve()), device="cpu")
            inference = torch.inference_mode
    print(json.dumps({"ready": True}), flush=True)
    for line in sys.stdin:
        try:
            payload = json.loads(line)
            with contextlib.redirect_stdout(sys.stderr), inference():
                result = agent.predict(payload["state"], payload["questions"])
            print(json.dumps(result, allow_nan=False), flush=True)
        except Exception:
            print('{"error":"inference_failed"}', flush=True)

if __name__ == "__main__":
    main()
