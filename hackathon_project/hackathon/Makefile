# LLM Gateway Integration — Makefile
# Track 2: Platform-Managed LLM Gateway (Nasiko Hackathon)

.PHONY: help start-gateway stop-gateway restart-gateway gateway-logs gateway-status
.PHONY: start-nasiko stop-nasiko build-agent agent-logs
.PHONY: init-gateway generate-key list-keys delete-key
.PHONY: test-agent-legacy test-agent-gateway integration-test
.PHONY: k8s-deploy k8s-delete k8s-logs clean shell-gateway db-shell

GATEWAY_COMPOSE  = docker/docker-compose.gateway.yml
LOCAL_COMPOSE    = ../docker-compose.local.yml
ENV_FILE         = .env.gateway
AGENT_DIR        = agents
SCRIPTS_DIR      = scripts

.DEFAULT_GOAL := help

# ── Help ──────────────────────────────────────────────────────────────────────

help:
	@echo ""
	@echo "NasikoGateway — LLM Router (Track 2)"
	@echo "======================================"
	@echo ""
	@echo "  Quick start (first time):"
	@echo "    1. cp env.gateway.example .env.gateway"
	@echo "    2. Edit .env.gateway with your API keys"
	@echo "    3. make start-nasiko"
	@echo "    4. make generate-key team=demo"
	@echo "    5. Add GATEWAY_VIRTUAL_KEY to .env.gateway"
	@echo "    6. make stop-nasiko && make start-nasiko"
	@echo "    7. make integration-test"
	@echo ""
	@echo "  Gateway only:"
	@echo "    make start-gateway      Start LiteLLM + Postgres + Redis"
	@echo "    make stop-gateway       Stop gateway stack"
	@echo "    make restart-gateway    Restart gateway"
	@echo "    make gateway-logs       Tail gateway logs"
	@echo "    make gateway-status     Health check"
	@echo ""
	@echo "  Full stack (gateway + agent):"
	@echo "    make start-nasiko       Start everything"
	@echo "    make stop-nasiko        Stop everything"
	@echo "    make build-agent        Rebuild agent image"
	@echo "    make agent-logs         Tail agent logs"
	@echo ""
	@echo "  Virtual keys:"
	@echo "    make generate-key team=<name>   Generate a virtual key"
	@echo "    make list-keys                  List all keys"
	@echo "    make delete-key key=<key>       Delete a key"
	@echo ""
	@echo "  Testing:"
	@echo "    make integration-test   Run end-to-end tests"
	@echo "    make test-agent-gateway Test gateway agent script"
	@echo "    make test-agent-legacy  Test legacy agent script"
	@echo ""
	@echo "  Cleanup:"
	@echo "    make clean              Remove all containers and volumes"
	@echo ""

# ── Gateway only ──────────────────────────────────────────────────────────────

start-gateway:
	@echo "Starting LiteLLM gateway stack..."
	@if [ ! -f $(ENV_FILE) ]; then echo "ERROR: $(ENV_FILE) not found. Run: cp env.gateway.example .env.gateway"; exit 1; fi
	docker compose --env-file $(ENV_FILE) -f $(GATEWAY_COMPOSE) up -d
	@echo "Waiting for gateway to be healthy..."
	@sleep 20
	@echo ""
	@echo "Gateway: http://localhost:4000"
	@echo "Admin UI: http://localhost:4000/ui"

stop-gateway:
	@echo "Stopping gateway stack..."
	docker compose -f $(GATEWAY_COMPOSE) down

restart-gateway: stop-gateway start-gateway

gateway-logs:
	docker compose -f $(GATEWAY_COMPOSE) logs -f litellm-gateway

gateway-status:
	@echo "--- Containers ---"
	@docker compose -f $(GATEWAY_COMPOSE) ps
	@echo ""
	@echo "--- Health ---"
	@curl -sf http://localhost:4000/health | python3 -m json.tool || echo "Gateway not responding"

start-nasiko:
	@echo "Starting LiteLLM gateway + Nasiko agent..."
	@if [ ! -f $(ENV_FILE) ]; then echo "ERROR: $(ENV_FILE) not found"; exit 1; fi
	docker compose --env-file $(ENV_FILE) -f $(LOCAL_COMPOSE) up -d --build
	@echo "Waiting 45s for gateway to boot..."
	@sleep 45
	@echo ""
	@curl -sf http://localhost:4000/health > /dev/null && echo "Gateway up" || echo "Gateway still starting — wait 30s more"
	@echo "Gateway: http://localhost:4000  |  Agent: http://localhost:8010"

stop-nasiko:
	@echo "Stopping all Nasiko services..."
	docker compose -f $(LOCAL_COMPOSE) down

build-agent:
	@echo "Rebuilding nasiko-agent image..."
	docker compose -f $(LOCAL_COMPOSE) build nasiko-agent

agent-logs:
	docker compose -f $(LOCAL_COMPOSE) logs -f nasiko-agent

# ── Virtual keys ──────────────────────────────────────────────────────────────

generate-key:
	@if [ -z "$(team)" ]; then echo "Usage: make generate-key team=<name>"; exit 1; fi
	@echo "Generating virtual key for team: $(team)"
	@TEAM="$(team)" \
	 MODELS="$(or $(models),gemini-flash,groq-llama3,gpt-3.5-turbo)" \
	 BUDGET="$(or $(budget),100.0)" \
	 bash $(SCRIPTS_DIR)/generate-virtual-key.sh

list-keys:
	@MASTER=$$(grep LITELLM_MASTER_KEY $(ENV_FILE) 2>/dev/null | cut -d= -f2); \
	curl -sf http://localhost:4000/key/list \
	     -H "Authorization: Bearer $$MASTER" | python3 -m json.tool || echo "Could not list keys"

delete-key:
	@if [ -z "$(key)" ]; then echo "Usage: make delete-key key=<key>"; exit 1; fi
	@MASTER=$$(grep LITELLM_MASTER_KEY $(ENV_FILE) 2>/dev/null | cut -d= -f2); \
	curl -sf -X POST http://localhost:4000/key/delete \
	     -H "Authorization: Bearer $$MASTER" \
	     -H "Content-Type: application/json" \
	     -d '{"key": "$(key)"}' | python3 -m json.tool

# ── Testing ───────────────────────────────────────────────────────────────────

integration-test:
	@echo "=== Integration Test (Track 2 Acceptance Criteria) ==="
	@PASS=true; \
	\
	printf "[1/4] Gateway health... "; \
	STATUS=$$(curl -s -o /dev/null -w "%{http_code}" http://localhost:4000/health); \
	if [ "$$STATUS" = "200" ]; then echo "✓"; else echo "✗ HTTP $$STATUS"; PASS=false; fi; \
	\
	printf "[2/4] Agent health...   "; \
	STATUS=$$(curl -s -o /dev/null -w "%{http_code}" http://localhost:8010/health); \
	if [ "$$STATUS" = "200" ]; then echo "✓"; else echo "✗ HTTP $$STATUS"; PASS=false; fi; \
	\
	printf "[3/4] Generating key... "; \
	MASTER=$$(grep LITELLM_MASTER_KEY $(ENV_FILE) 2>/dev/null | cut -d= -f2); \
	VKEY=$$(curl -s -X POST http://localhost:4000/key/generate \
	        -H "Authorization: Bearer $$MASTER" \
	        -H "Content-Type: application/json" \
	        -d '{"models":["gemini-flash","groq-llama3"],"duration":"1h"}' \
	        | python3 -c "import sys,json; print(json.load(sys.stdin).get('key',''))" 2>/dev/null); \
	if [ -n "$$VKEY" ]; then echo "✓  $${VKEY:0:15}..."; else echo "✗ Failed"; PASS=false; fi; \
	\
	printf "[4/4] Agent chat...     "; \
	RESP=$$(curl -s -X POST http://localhost:8010/chat \
	        -H "Content-Type: application/json" \
	        -d '{"message":"Say hello in exactly one word."}'); \
	if echo "$$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); exit(0 if d.get('response') else 1)" 2>/dev/null; \
	then echo "✓"; else echo "✗  $$RESP"; PASS=false; fi; \
	\
	echo ""; \
	if [ "$$PASS" = "true" ]; then echo "✅  ALL TESTS PASSED"; else echo "❌  SOME TESTS FAILED"; exit 1; fi

test-agent-gateway:
	@echo "Testing gateway agent script..."
	cd $(AGENT_DIR) && python3 sample-agent-gateway.py

test-agent-legacy:
	@echo "Testing legacy agent script..."
	cd $(AGENT_DIR) && python3 sample-agent-legacy.py

# ── Kubernetes ────────────────────────────────────────────────────────────────

k8s-deploy:
	kubectl apply -f k8s/secrets.yaml
	kubectl apply -f k8s/gateway-deployment.yaml
	kubectl get pods -l app=litellm-gateway

k8s-delete:
	kubectl delete -f k8s/gateway-deployment.yaml
	kubectl delete -f k8s/secrets.yaml

k8s-logs:
	kubectl logs -l app=litellm-gateway --tail=100 -f

# ── Cleanup ───────────────────────────────────────────────────────────────────

clean:
	@echo "Removing all containers and volumes..."
	-docker compose -f $(LOCAL_COMPOSE) down -v 2>/dev/null
	-docker compose -f $(GATEWAY_COMPOSE) down -v 2>/dev/null
	@echo "Done."

# ── Dev helpers ───────────────────────────────────────────────────────────────

shell-gateway:
	docker compose -f $(GATEWAY_COMPOSE) exec litellm-gateway /bin/sh

db-shell:
	docker compose -f $(GATEWAY_COMPOSE) exec postgres psql -U litellm -d litellm
