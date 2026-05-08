# LLM Router Gateway Integration Design

## Rationale: LiteLLM vs Portkey
For Track 2, we have selected **LiteLLM** as the platform-managed LLM gateway. 

**Trade-offs & Rationale:**
* **LiteLLM Advantages:** It is open-source, easily containerized via `ghcr.io`, and provides a seamless unified API format (OpenAI format) for over 100+ providers. It runs natively in-cluster without external dependencies.
* **Portkey Disadvantages:** While Portkey offers superior enterprise observability UI, its self-hosted open-source version is heavier and requires more complex database provisioning compared to LiteLLM's lightweight stateless proxy design.
* **Decision:** LiteLLM aligns perfectly with Nasiko's need for a lightweight, in-cluster routing layer that won't bloat the local deployment.

## Implementation Steps (Track 2)
1. **Infra Deployment:** `litellm-gateway` added to `docker-compose.local.yml` running on port 4000.
2. **Provider Credentials:** Managed centrally via `litellm_config.yaml`.
3. **WARNING TO AGENT DEVELOPERS:** Do NOT hardcode provider keys (like `OPENAI_API_KEY`) in your agent source code. You must route requests to `http://litellm-gateway:4000` using the platform-injected virtual key.

## 🔐 Virtual Key Provisioning & Rotation Model (Production Design)
While the local docker-compose environment utilizes a static dev key (`sk-nasiko-local-dev-key`), the production Kubernetes environment will implement the following lifecycle:

### 1. Minting
* Virtual keys are NOT mapped 1:1 to users; they are mapped 1:1 to **Agent Deployments**.
* Upon successful agent build, the Nasiko Backend calls the Auth Service to cryptographically generate a unique `sk-nasiko-v1-[uuid]` key.
* The key scope is strictly bound to the assigned LiteLLM routing profile for that specific agent.

### 2. Storage
* **Gateway Side:** The plaintext virtual key is injected into the LiteLLM PostgreSQL database as a valid token.
* **Agent Side:** The key is stored as a Kubernetes Secret. The orchestrator (`agent_builder.py`) mounts this secret into the agent's pod as the `OPENAI_API_KEY` environment variable. It is NEVER stored in plaintext in the agent's source code or the main Nasiko database.

### 3. Rotation Policy
* **Manual Rotation:** Developers can trigger rotation via the CLI (`nasiko agent rotate-keys my-agent`). 
* **Automated Rotation:** By default, keys are marked for 90-day rotation.
* **Execution:** When rotation triggers, the Backend generates a new key, updates the K8s Secret, and triggers a zero-downtime rolling restart of the agent's pods. The old key is immediately invalidated in the LiteLLM database.
