# Makefile for local development
# Mirrors the pattern from the main Nasiko Makefile at repo root
# Usage: make start-local   (or equivalent of make start-nasiko for our module)

.PHONY: help test demo start-local stop-local clean lint

# Default target
help:
	@echo "Available targets:"
	@echo "  start-local    - Start all services (Phoenix, Kong, LiteLLM Gateway, Nasiko Server)"
	@echo "  stop-local     - Stop all services"
	@echo "  test           - Run all 106 tests"
	@echo "  demo           - Run the local demo (no Docker needed)"
	@echo "  lint           - Run code formatting check"
	@echo "  clean          - Remove temp files and caches"

# Start all services via docker-compose (equivalent of make start-nasiko)
start-local:
	@echo "Starting Nasiko MCP services (Phoenix + Kong + LLM Gateway + Server)..."
	docker compose -f nasiko/docker-compose.local.yml up -d
	@echo ""
	@echo "Waiting for services to be healthy..."
	@sleep 10
	@echo ""
	@echo "Services started:"
	@echo "  Phoenix UI:    http://localhost:6006"
	@echo "  Kong Admin:    http://localhost:8001"
	@echo "  LLM Gateway:   http://localhost:4000"
	@echo "  Nasiko Server: http://localhost:8000"
	@echo ""
	@echo "Run 'make demo' or 'bash demo/run_demo.sh' to test the pipeline."

# Stop all services
stop-local:
	@echo "Stopping Nasiko MCP services..."
	docker compose -f nasiko/docker-compose.local.yml down
	@echo "Services stopped."

# Run all tests (no Docker needed)
test:
	python -m pytest tests/ -v

# Run the local demo (no Docker needed)
demo:
	python demo/demo_local.py

# Lint check (matches upstream CI)
lint:
	python -m black --check .

# Clean
clean:
	@echo "Cleaning temp files..."
	-rm -rf __pycache__ .pytest_cache
	-find . -type d -name "__pycache__" -exec rm -rf {} + 2>/dev/null || true
	-find . -type d -name ".pytest_cache" -exec rm -rf {} + 2>/dev/null || true
	@echo "Clean complete."
