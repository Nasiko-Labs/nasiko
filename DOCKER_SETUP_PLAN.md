# Nasiko Local Docker Setup Plan

## Objective
Enable a complete local development environment for the Nasiko platform using **Docker and Docker Compose** exclusively. This replaces the dependency on the Kubernetes-based infrastructure (including the Kubernetes-based BuildKit deployment) for local users, offering a faster and lighter alternative.

## Core Strategy
1.  **Registry/Discovery**: Modify the existing `kong-service-registry` to support **Docker Service Discovery** (via `docker.sock`) in addition to Kubernetes API discovery.
2.  **Orchestration/Builds**: dedicated "Worker" service running `orchestrator/redis_stream_listener.py` inside a custom Docker image with Docker CLI installed.
3.  **Unified Stack**: Consolidate disparate `docker-compose.yml` files (Gateway, App, Web) into a single master orchestration file (`docker-compose.nasiko.yml`) using a single shared network.
4.  **CLI Integration**: Add a new `nasiko local` command group to manage this stack.

---

## Phase 1: Registry & Discovery Updates

The current `agent-gateway/registry/registry.py` only knows how to talk to Kubernetes. We must teach it to talk to Docker.

### 1.1 Update `agent-gateway/registry/requirements.txt`
*   Add `docker>=7.0.0` to support the Python Docker client.

### 1.2 Modify `agent-gateway/registry/registry.py`
*   **Feature Flag**: Introduce `DISCOVERY_MODE` env var (values: `kubernetes` or `docker`).
*   **Docker Client**: Initialize `docker.from_env()` when in Docker mode.
*   **Discovery Logic**: Implement `get_docker_services()`:
    *   List containers attached to the `nasiko-network` (unified network).
    *   Filter for containers with specific labels (e.g., `nasiko.type=agent`) AND fallback to naming conventions for static services.
    *   Extract IP addresses and exposed ports directly from container inspection.
*   **Integration**: Switch `sync_services()` loop to call `get_docker_services()` when configured.

### 1.3 Update `orchestrator/agent_builder.py`
*   **Label Injection**: Modify `_deploy_agent` to inject discovery labels into the generated agent `docker-compose.yml`:
    *   `nasiko.type=agent`
    *   `nasiko.name={agent_name}`
    *   `nasiko.owner={owner_id}`

---

## Phase 2: Unified Docker Compose Configuration

We need a single "Source of Truth" compose file that spins up the Core Platform and the Worker.

### 2.1 Create `Dockerfile.worker` (in `orchestrator/` or root)
*   **Base Image**: `python:3.12-slim` (matching project standard).
*   **Dependencies**:
    *   Install Docker CLI and Docker Compose plugin (via `apt` or `COPY --from docker`).
    *   Install Python dependencies from `pyproject.toml` (specifically `redis`, `aiohttp`, `pyyaml`, `docker`).
*   **Code**: Copy `orchestrator/` code into the image.
*   **Entrypoint**: `python orchestrator/redis_stream_listener.py`.

### 2.2 Create `docker-compose.nasiko.yml` (Root Directory)
This file will merge services, **correcting all relative paths** (e.g., `./plugins` -> `./agent-gateway/plugins`):
1.  **Gateway Stack**:
    *   `kong`, `kong-database`, `kong-migrations`, `konga`
    *   `kong-service-registry` (`DISCOVERY_MODE=docker`, `AGENTS_NETWORK=nasiko-network`)
    *   `nasiko-router`
    *   `nasiko-auth-service`, `auth-redis`
    *   `chat-history-service`
2.  **App Stack**:
    *   `nasiko-backend` (Environment `REDIS_HOST=redis`)
    *   `mongodb`
    *   `redis` (Exposed for Worker)
3.  **Web Stack**:
    *   `nasiko-web`
4.  **Worker Service** (`nasiko-worker`):
    *   **Build**: Uses new `Dockerfile.worker`.
    *   **Volumes**:
        *   `/var/run/docker.sock:/var/run/docker.sock`
        *   `./agents:/app/agents`
    *   **Environment**: `REDIS_HOST=redis`, `DOCKER_NETWORK=nasiko-network`.

### 2.3 Network Unification
*   **Global Rename**: Standardize on `nasiko-network` for everything.
*   **Update Code**:
    *   `orchestrator/config.py`: Set `DOCKER_NETWORK = "nasiko-network"`.
    *   `orchestrator/agent_builder.py`: Ensure injected network matches `nasiko-network`.
*   **Compose Definition**:
    *   In `docker-compose.nasiko.yml`: Define `nasiko-network` as a standard bridge network (NOT external).
    *   In generated agent compose files: Define `nasiko-network` as `external: true`.

---

## Phase 3: CLI Implementation

Automate the setup so users don't have to manually run `docker-compose -f ...`.

### 3.1 Create `cli/groups/local_group.py`
*   **Command**: `nasiko local up`
*   **Logic**:
    *   Check for running Docker daemon.
    *   Check if `nasiko-network` exists (optional, as main compose handles it, but good for validation).
    *   Run `docker compose -f docker-compose.nasiko.yml up -d --build`.
    *   Stream logs or wait for health checks.
*   **Command**: `nasiko local down`
    *   Stops the stack.

### 3.2 Update `cli/main.py`
*   Register the new group: `from groups.local_group import local_app` and `app.add_typer(local_app, name="local")`.

---

## Phase 4: Verification Plan

1.  **Start Stack**: Run `nasiko local up`.
2.  **Verify Discovery**: Check `http://localhost:8080/status` (Registry) to ensure it sees the static proxies.
3.  **Deploy Agent**: Use the Nasiko UI or API to deploy a test agent.
    *   *Expectation*: The `nasiko-worker` (running in Docker) picks up the job.
    *   *Expectation*: The worker executes `docker build` and `docker compose up`.
    *   *Expectation*: The new agent container appears attached to `nasiko-network`.
    *   *Expectation*: `docker inspect` shows the agent has `nasiko.type=agent` label.
4.  **Verify Registration**: The Registry detects the new container (via label/network) and registers it.
5.  **End-to-End**: Chat with the agent via the Router/Gateway.

/orchestrator/ is purely for local Docker development.

  Evidence:

  1. All Docker-based — docker_utils.py wraps docker CLI commands (docker inspect, subprocess.run), agent_builder.py uses Docker networks and docker-compose
  2. No K8s references — Nothing in the orchestrator uses Kubernetes APIs, kubectl, or K8s SDKs
  3. Not imported by the backend — No from orchestrator or import orchestrator anywhere in /app/
  4. Not deployed to K8s — No Dockerfile, no K8s manifests (YAML), no Helm chart for the orchestrator itself
  5. Not referenced by CLI setup — /cli/setup/ has zero references to the orchestrator
  6. Separate consumer groups — The orchestrator's redis_stream_listener.py uses consumer group "orchestrator", while the K8s worker uses "k8s-orchestrator" — they're independent consumers

  The two paths are completely separate:
  - Local dev: orchestrator/ → Docker containers
  - Production K8s: cli/setup/ (infra) + worker/k8s_build_worker.py (runtime agent deployments)
