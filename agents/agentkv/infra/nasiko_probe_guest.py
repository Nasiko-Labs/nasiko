"""Runs inside a disposable Modal VM. Credentials never leave the VM."""
import base64
import json
import os
from pathlib import Path
import secrets
import subprocess
import time
import urllib.request

NASIKO_COMMIT = "58cfe600559c67d58100ec2856d7b29838e2859f"
NASIKO_IMAGE = "agentkv-nasiko:" + NASIKO_COMMIT[:12]


REDACTIONS = set()


def command(*args, timeout=180):
    result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
    if result.returncode:
        detail = result.stderr.decode(errors="replace")
        for value in sorted(REDACTIONS, key=len, reverse=True):
            detail = detail.replace(value, "[redacted]")
        raise RuntimeError(f"Command failed: {args[0]} {args[1]}: {detail[-1500:]}")
    return result.stdout.decode()


def env_file(name, values):
    REDACTIONS.update(str(v) for v in values.values() if len(str(v)) >= 16)
    path = "/tmp/" + name + ".env"
    descriptor = os.open(path, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        stream.write("\n".join(f"{k}={v}" for k, v in values.items()))
    return path


def request(path, body=None, token=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    req = urllib.request.Request("http://127.0.0.1:8080" + path,
        data=json.dumps(body).encode() if body else None, headers=headers)
    with urllib.request.urlopen(req, timeout=5) as response:
        if path == "/health":
            if response.read().strip() != b"ok":
                raise ValueError("unexpected health response")
            return {"healthy": True}
        return json.load(response)


def main():
    for attempt in range(40):
        if subprocess.run(["docker", "info"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
            break
        time.sleep(2)
    else:
        raise RuntimeError("Docker unavailable")
    cache_root = os.getenv("AGENTKV_BUILD_CACHE")
    cache = Path(cache_root, NASIKO_COMMIT + ".tar") if cache_root else None
    if cache and cache.exists():
        print("Loading cached, pinned Nasiko dependency image", flush=True)
        command("docker", "load", "-i", str(cache), timeout=180)
    else:
        print("Building unmodified pinned Nasiko dependency", flush=True)
        command("git", "clone", "--filter=blob:none", "https://github.com/Nasiko-Labs/nasiko.git", "/tmp/nasiko")
        command("git", "-C", "/tmp/nasiko", "checkout", "--detach", NASIKO_COMMIT)
        command("env", "DOCKER_BUILDKIT=1", "docker", "build", "-t", NASIKO_IMAGE,
                "-f", "/tmp/nasiko/server/Dockerfile", "/tmp/nasiko", timeout=1200)
        if cache:
            temporary = cache.with_suffix(".partial")
            command("docker", "save", "-o", str(temporary), NASIKO_IMAGE, timeout=180)
            temporary.replace(cache)
            command("sync", str(cache.parent))
    print("Nasiko image built; starting backing services", flush=True)
    command("docker", "network", "create", "nasiko")
    # A private trace backend gives Nasiko real flow/span inspection.
    Path('/tmp/tempo.yaml').write_text("""server:
  http_listen_port: 3200
distributor:
  receivers:
    otlp:
      protocols:
        grpc:
          endpoint: 0.0.0.0:4317
storage:
  trace:
    backend: local
    wal:
      path: /tmp/tempo/wal
    local:
      path: /tmp/tempo/blocks
usage_report:
  reporting_enabled: false
""")
    command('docker','run','-d','--name','tempo','--network','nasiko',
            '-p','127.0.0.1:3200:3200','-v','/tmp/tempo.yaml:/etc/tempo.yaml:ro',
            'grafana/tempo:2.6.1','-config.file=/etc/tempo.yaml')
    password = secrets.token_urlsafe(32)
    admin = secrets.token_urlsafe(32)
    storage = secrets.token_urlsafe(32)
    postgres = env_file("postgres", {"POSTGRES_USER": "nasiko", "POSTGRES_PASSWORD": password,
                                     "POSTGRES_DB": "nasiko_dev"})
    command("docker", "run", "-d", "--name", "postgres", "--network", "nasiko",
            "--env-file", postgres, "pgvector/pgvector:pg16")
    command("docker", "run", "-d", "--name", "redis", "--network", "nasiko", "redis:7-alpine")
    s3 = env_file("storage", {"RUSTFS_ROOT_USER": "nasiko", "RUSTFS_ROOT_PASSWORD": storage})
    command("docker", "run", "-d", "--name", "rustfs", "--network", "nasiko",
            "--env-file", s3, "rustfs/rustfs:latest", "server", "/data", "--address", ":9000")
    for attempt in range(40):
        process = subprocess.run(["docker", "exec", "postgres", "pg_isready", "-U", "nasiko"],
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if process.returncode == 0:
            break
        time.sleep(1)
    command("docker", "exec", "postgres", "createdb", "-U", "nasiko", "nasiko_registry")
    server_settings = {
        "DATABASE_URL": f"postgres://nasiko:{password}@postgres:5432/nasiko_dev",
        "REDIS_URL": "redis://redis:6379", "JWT_SECRET": secrets.token_urlsafe(48),
        "AGENT_JWT_SECRET": secrets.token_urlsafe(48),
        "SECRETS_ENCRYPTION_KEY": base64.b64encode(secrets.token_bytes(32)).decode(),
        "ADMIN_USERNAME": "agentkv", "ADMIN_PASSWORD": admin, "ADMIN_EMAIL": "demo@example.invalid",
        "S3_ENDPOINT": "http://rustfs:9000", "S3_BUCKET": "nasiko", "S3_REGION": "us-east-1",
        "S3_ACCESS_KEY": "nasiko", "S3_SECRET_KEY": storage, "AGENT_RUNTIME": "docker",
        "DOCKER_AGENT_NETWORK": "nasiko", "MCP_GATEWAY_PUBLIC_URL": "http://server:8080/api/mcp",
        "LLM_GATEWAY_BASE_URL": "http://server:8080", "RUST_LOG": "warn,nasiko_server=info,nasiko_llm_router=info",
        "TEMPO_URL":"http://tempo:3200", "OTEL_EXPORTER_OTLP_ENDPOINT":"http://tempo:4317",
        "OTEL_COLLECTOR_ENDPOINT":"http://tempo:4317", "OTEL_EXPORTER_OTLP_PROTOCOL":"grpc",
        # Provider settings are present only during an explicitly launched demo.
    }
    if os.getenv("AGENTKV_PROVIDER_URL"):
        server_settings.update({
            "OPENAI_API_BASE": os.environ["AGENTKV_PROVIDER_URL"].rstrip("/") + "/v1",
            "PLATFORM_OPENAI_API_KEY": os.environ["AGENTKV_PROVIDER_KEY"],
            "DEFAULT_MODEL": "agentkv-qwen", "OPENAI_MODEL": "agentkv-qwen",
        })
    server = env_file("server", server_settings)
    command("docker", "run", "-d", "--name", "server", "--network", "nasiko",
            "--env-file", server, "-p", "8080:8080" if os.getenv("AGENTKV_SHOW_UI")=="1" else "127.0.0.1:8080:8080",
            "-v", "/var/run/docker.sock:/var/run/docker.sock", NASIKO_IMAGE)
    for attempt in range(60):
        try:
            request("/health")
            break
        except Exception:
            time.sleep(2)
    else:
        state = json.loads(command("docker", "inspect", "server"))[0]["State"]
        raise RuntimeError("Nasiko health failed; running=" + str(state["Running"]))
    login = request("/api/auth/login", {"username": "agentkv", "password": admin})
    request("/api/me", token=login["token"])
    images = {name: json.loads(command("docker", "inspect", name))[0]["Image"]
              for name in ["postgres", "redis", "rustfs", "server", "tempo"]}
    Path("/tmp/nasiko-evidence.json").write_text(json.dumps({"upstream_revision":NASIKO_COMMIT,"dependency_image_ids":images}))
    print(json.dumps({"nasiko_health": True, "authenticated_api": True, "image": NASIKO_IMAGE,
                      "upstream_revision": NASIKO_COMMIT,
                      "dependency_image_ids": images}))


if __name__ == "__main__":
    main()
