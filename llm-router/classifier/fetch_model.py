"""Explicit online preparation. Inference uses only the resulting local directory."""
import argparse
import hashlib
import json
from pathlib import Path
from huggingface_hub import HfApi, snapshot_download

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--repo", default="convaiinnovations/laya")
    p.add_argument("--revision", default="55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851")
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / ".gitignore").write_text("*\n", encoding="utf-8")
    info = HfApi().model_info(args.repo, revision=args.revision)
    patterns = ["rl_agent_config.json", "model.safetensors", "encoder/*.json", "tokenizer/*"]
    if args.repo.startswith("sentence-transformers/"):
        patterns = ["model.safetensors", "config.json", "tokenizer.json", "tokenizer_config.json", "special_tokens_map.json", "vocab.txt"]
    snapshot_download(args.repo, revision=info.sha, local_dir=str(args.output), allow_patterns=patterns)
    manifest = {"repo": args.repo, "revision": info.sha, "files": {}}
    for path in sorted(args.output.rglob("*")):
        if path.is_file() and ".cache" not in path.parts and path.name not in {"provenance.json", ".gitignore"}:
            digest = hashlib.sha256()
            with path.open("rb") as f:
                for block in iter(lambda: f.read(1024 * 1024), b""):
                    digest.update(block)
            manifest["files"][str(path.relative_to(args.output)).replace("\\", "/")] = digest.hexdigest()
    (args.output / "provenance.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    print(json.dumps({"repo": args.repo, "revision": info.sha, "files": len(manifest["files"])}))

if __name__ == "__main__":
    main()
