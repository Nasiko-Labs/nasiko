"""Pinned weights download and a bounded GPU compatibility gate."""
from pathlib import Path
import json
import modal

MODEL = "Qwen/Qwen3.8-27B"
REVISION = "1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0"
MODEL_PATH = "/models/qwen3.8-27b"
app = modal.App("agentkv-model-gate")
weights = modal.Volume.from_name("agentkv-model-weights", create_if_missing=True)
outputs = modal.Volume.from_name("agentkv-results", create_if_missing=True)
download_image = modal.Image.debian_slim(python_version="3.12").pip_install("huggingface_hub==1.32.0")
gpu_base_image = (modal.Image.debian_slim(python_version="3.12")
    .pip_install("vllm==0.29.0")
    .apt_install("build-essential")
    .env({"VLLM_WORKER_MULTIPROC_METHOD": "spawn", "HF_HUB_OFFLINE": "1", "VLLM_USE_FLASHINFER_SAMPLER": "0",
          "CUDA_HOME": "/usr/local/lib/python3.12/site-packages/nvidia/cu13"}))
gpu_image = gpu_base_image.add_local_python_source("agentkv")


@app.function(image=download_image, volumes={"/models": weights}, cpu=4, memory=8192,
              timeout=1200, retries=0, max_containers=1)
def download():
    from huggingface_hub import snapshot_download
    snapshot_download(MODEL, revision=REVISION, local_dir=MODEL_PATH,
                      allow_patterns=["*.json", "*.safetensors", "*.jinja", "*.txt", "*.model"], max_workers=8)
    Path(MODEL_PATH, "agentkv-revision.txt").write_text(REVISION)
    weights.commit()
    return {"model": MODEL, "revision": REVISION, "bytes": sum(p.stat().st_size for p in Path(MODEL_PATH).rglob("*.safetensors"))}


@app.function(image=gpu_image, gpu="A100-80GB", cpu=8, memory=65536,
              volumes={"/models": weights, "/results": outputs},
              timeout=1200, retries=0, max_containers=1)
def smoke():
    import time
    from vllm import LLM, SamplingParams
    started = time.time()
    engine = LLM(model=MODEL_PATH, dtype="bfloat16", max_model_len=8192,
                 max_num_seqs=4, gpu_memory_utilization=.88, enforce_eager=True,
                 enable_prefix_caching=True, language_model_only=True,
                 max_num_batched_tokens=2048, seed=7)
    ready = time.time()
    tokenizer = engine.get_tokenizer()
    prompt = tokenizer.apply_chat_template([
        {"role": "system", "content": "You are a careful Python coding assistant."},
        {"role": "user", "content": "Write a Python function add(a, b) that returns their sum. Return only code."}
    ], tokenize=False, add_generation_prompt=True, enable_thinking=False)
    runs = []
    for _ in range(2):
        start = time.perf_counter()
        result = engine.generate([prompt], SamplingParams(temperature=0, max_tokens=80), use_tqdm=False)[0]
        runs.append({"seconds": time.perf_counter()-start, "text": result.outputs[0].text,
                     "prompt_tokens": len(result.prompt_token_ids),
                     "cached_tokens": result.num_cached_tokens,
                     "output_tokens": len(result.outputs[0].token_ids)})
    report = {"model": MODEL, "revision": REVISION, "vllm": "0.29.0", "gpu": "A100-80GB",
              "startup_seconds": ready-started, "runs": runs, "total_seconds": time.time()-started}
    Path("/results/model-gate.json").write_text(json.dumps(report, indent=2))
    outputs.commit()
    return report


@app.local_entrypoint()
def main(mode: str = "download"):
    report = download.remote() if mode == "download" else smoke.remote()
    Path("results").mkdir(exist_ok=True)
    Path(f"results/{mode}.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
