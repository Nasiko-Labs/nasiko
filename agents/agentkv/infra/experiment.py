"""Bounded all-Modal integration run. CPU VM owns Nasiko and agents; GPU is separate."""
import json
import hashlib
import os
from pathlib import Path
import secrets
import modal
from infra.model import gpu_base_image, weights, MODEL_PATH, REVISION

app = modal.App("agentkv-experiment")
build_cache = modal.Volume.from_name("agentkv-build-cache", create_if_missing=True, version=2)
provider_key = secrets.token_urlsafe(48)
provider_secret = modal.Secret.from_dict({"AGENTKV_PROVIDER_KEY": provider_key})


@app.function(image=gpu_base_image.env({"AGENTKV_CACHE_BYTES":os.getenv("AGENTKV_CACHE_BYTES","0")}).add_local_python_source("agentkv", "infra"), gpu="A100-80GB", cpu=8, memory=65536,
    volumes={"/models": weights}, secrets=[provider_secret],
    timeout=7200, max_containers=1, scaledown_window=60)
@modal.concurrent(max_inputs=16)
@modal.web_server(8765, startup_timeout=900)
def serve():
    import subprocess
    import sys
    import time
    import urllib.request
    if Path(MODEL_PATH,"agentkv-revision.txt").read_text().strip()!=REVISION:
        raise RuntimeError("Model volume revision mismatch; run pinned download")
    os.environ["VLLM_SERVER_DEV_MODE"] = "1"
    os.environ["AGENTKV_GPU_STARTED_AT"] = str(time.time())
    commands = [sys.executable, "-m", "vllm.entrypoints.openai.api_server",
        "--model", MODEL_PATH, "--served-model-name", "agentkv-qwen",
        "--host", "127.0.0.1", "--port", "8000", "--dtype", "bfloat16",
        "--max-model-len", "4096" if int(os.getenv("AGENTKV_CACHE_BYTES","0")) else "8192", "--max-num-seqs", "4",
        "--max-num-batched-tokens", "2048", "--gpu-memory-utilization", "0.88",
        "--enable-prefix-caching", "--language-model-only", "--enforce-eager",
        "--scheduler-cls", "agentkv.engine.AgentKVScheduler",
        "--enable-prompt-tokens-details", "--seed", "7"]
    if int(os.getenv("AGENTKV_CACHE_BYTES","0")):
        commands += ["--kv-cache-memory-bytes",os.environ["AGENTKV_CACHE_BYTES"]]
    server = subprocess.Popen(["timeout", "6600", *commands])
    for _ in range(450):
        if server.poll() is not None:
            raise RuntimeError("vLLM failed to start")
        try:
            urllib.request.urlopen("http://127.0.0.1:8000/health", timeout=2).close()
            break
        except Exception:
            time.sleep(2)
    else:
        server.terminate()
        raise RuntimeError("vLLM readiness deadline exceeded")
    subprocess.Popen(["timeout", "6600", sys.executable, "-m", "uvicorn", "agentkv.gateway:app",
                      "--host", "0.0.0.0", "--port", "8765", "--no-access-log"])


cpu_image = (modal.Image.debian_slim(python_version="3.12").apt_install("docker.io", "git")
    .pip_install("httpx==0.28.1", "typesafe-sdk==0.7.0", "pydantic==2.13.5", "modal==1.5.0")
    .add_local_dir("src/agentkv", "/app/agentkv")
    .add_local_dir("agents/python", "/app/agent-package")
    .add_local_dir("benchmarks", "/app/benchmarks")
    .add_local_file("infra/nasiko_probe_guest.py", "/app/nasiko_host.py")
    .add_local_file("infra/demo_guest.py", "/app/demo_guest.py"))


@app.local_entrypoint()
def main(limit: int = 2, methods: str = "A,B", concurrency: int = 1, repeats: int = 1, show_ui: bool = False, hold_seconds: int = 0):
    import time
    if not 1 <= limit <= 20 or concurrency not in (1,4) or not 1 <= repeats <= 3:
        raise ValueError("Bounded pilot: 1–20 tasks, concurrency 1/4, 1–3 repeats")
    if not 0 <= hold_seconds <= 900 or not set(methods.split(',')) <= set('ABCDE'):
        raise ValueError("Invalid methods or demo hold (maximum 900 seconds)")
    if os.getenv("AGENTKV_WORKFLOW","coding") not in {"coding","support"}:
        raise ValueError("AGENTKV_WORKFLOW must be coding or support")
    if os.getenv("AGENTKV_WARMING","1") not in {"0","1"}:
        raise ValueError("AGENTKV_WARMING must be 0 or 1")
    if os.getenv("AGENTKV_HARNESS","pi") not in {"pi","direct"}:
        raise ValueError("AGENTKV_HARNESS must be pi or direct")
    source_hashes={str(p):hashlib.sha256(p.read_bytes()).hexdigest()
        for folder in ('src/agentkv','infra','agents/python','benchmarks')
        for p in Path(folder).rglob('*') if p.is_file() and '__pycache__' not in str(p)
        and p.suffix in ('.py','.json')}
    started = time.time()
    sandbox = modal.Sandbox.create("bash", "-lc", "dockerd >/tmp/docker.log 2>&1 & wait",
        app=app, image=cpu_image, cpu=4, memory=8192, timeout=7200,
        experimental_options={"vm_runtime": True}, encrypted_ports=[8080] if show_ui else [], volumes={"/build-cache": build_cache},
        secrets=[provider_secret, modal.Secret.from_name("agentkv-jev")],
        env={"AGENTKV_SHOW_UI":"1" if show_ui else "0", "AGENTKV_CACHE_BYTES":os.getenv("AGENTKV_CACHE_BYTES","0"), "AGENTKV_BUILD_CACHE":"/build-cache", "PYTHONPATH": "/app", "AGENTKV_PROVIDER_URL": serve.get_web_url(),
             "AGENTKV_TASK_LIMIT": str(limit), "AGENTKV_METHODS": methods,
             "AGENTKV_CONCURRENCY": str(concurrency), "AGENTKV_REPEATS": str(repeats),
             "AGENTKV_WORKFLOW": os.getenv("AGENTKV_WORKFLOW","coding"),
             "AGENTKV_WARMING": os.getenv("AGENTKV_WARMING","1"),
             "AGENTKV_HARNESS": os.getenv("AGENTKV_HARNESS","pi")})
    print("Experiment sandbox:", sandbox.object_id, flush=True)
    demo_url=sandbox.tunnels()[8080].url if show_ui else None
    try:
        process = sandbox.exec("python", "-u", "/app/demo_guest.py", timeout=6900)
        for line in process.stdout:
            print(line, end="", flush=True)
            if show_ui and '"nasiko_health": true' in line:
                credentials=sandbox.exec('python','-c',
                    "import json; from pathlib import Path; e=dict(x.split('=',1) for x in Path('/tmp/server.env').read_text().splitlines()); print(json.dumps({'username':'agentkv','password':e['ADMIN_PASSWORD']}))")
                credentials.wait()
                access=json.loads(credentials.stdout.read()); access['url']=demo_url
                Path('results').mkdir(exist_ok=True)
                descriptor=os.open('results/demo-access.json',os.O_CREAT|os.O_WRONLY|os.O_TRUNC,0o600)
                with os.fdopen(descriptor,'w') as stream: json.dump(access,stream)
                print('Nasiko UI:',demo_url,'(login saved privately in results/demo-access.json)',flush=True)
        process.wait()
        if process.returncode:
            print(process.stderr.read()[-4000:])
            raise RuntimeError("Experiment failed; see bounded diagnostic above")
        result = sandbox.exec("cat", "/tmp/agentkv-report.json")
        result.wait()
        payload = json.loads(result.stdout.read())
        payload["cpu_sandbox_wall_seconds"] = time.time() - started
        payload["source_hashes"]=source_hashes
        payload["modal_app_id"]=app.app_id
        Path("results").mkdir(exist_ok=True)
        encoded=json.dumps(payload,indent=2)
        Path("results/integrated-latest.json").write_text(encoded)
        Path(f"results/run-{int(started)}-c{concurrency}.json").write_text(encoded)
        print("Results saved to results/integrated-latest.json")
        if hold_seconds:
            print(f'Demo available for {hold_seconds} seconds before teardown',flush=True)
            time.sleep(hold_seconds)
    finally:
        sandbox.terminate()
        print("Nasiko VM terminated; Modal run teardown stops the GPU service")
