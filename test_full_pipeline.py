version: '3.8'

services:
  # ── R5: Phoenix Observability UI ──────────────────────────────────
  # All OpenTelemetry traces land here. Access the UI at http://localhost:6006
  phoenix-observability:
    image: arizephoenix/phoenix:latest
    ports:
      - "6006:6006"   # Phoenix web UI
      - "4317:4317"   # OTLP gRPC receiver
      - "4318:4318"   # OTLP HTTP receiver
    environment:
      - PHOENIX_WORKING_DIR=/phoenix_data
    volumes:
      - phoenix_data:/phoenix_data
    networks:
      - nasiko_net
    healthcheck:
      test: ["CMD", "python", "-c", "import urllib.request; urllib.request.urlopen('http://localhost:6006')"]
      interval: 10s
      timeout: 5s
      retries: 5

  # ── Kong API Gateway ──────────────────────────────────────────────
  # R2 registers bridge services here. Admin API at http://localhost:8001
  kong:
    image: kong:3.6-ubuntu
    environment:
      - KONG_DATABASE=off
      - KONG_DECLARATIVE_CONFIG=/dev/null
      - KONG_PROXY_ACCESS_LOG=/dev/stdout
      - KONG_ADMIN_ACCESS_LOG=/dev/stdout
      - KONG_PROXY_ERROR_LOG=/dev/stderr
      - KONG_ADMIN_ERROR_LOG=/dev/stderr
      - KONG_ADMIN_LISTEN=0.0.0.0:8001
      - KONG_PROXY_LISTEN=0.0.0.0:8000
    ports:
      - "8001:8001"   # Kong Admin API (R2 registers services here)
      - "8002:8000"   # Kong Proxy (agents call tools through here)
    networks:
      - nasiko_net
    healthcheck:
      test: ["CMD", "kong", "health"]
      interval: 10s
      timeout: 5s
      retries: 5

  # ── LiteLLM Gateway ──────────────────────────────────────────────
  # Intercepts LLM calls from uploaded agents, routing through Nasiko's proxy
  llm-gateway:
    image: ghcr.io/berriai/litellm:main-latest
    ports:
      - "4000:4000"
    volumes:
      - ./litellm-config.yaml:/app/config.yaml
    command: ["--config", "/app/config.yaml", "--port", "4000"]
    environment:
      - LITELLM_MASTER_KEY=nasiko-virtual-proxy-key
    networks:
      - nasiko_net

  # ── Nasiko MCP Bridge Server (R1 + R2 + R3 + R4 + R5) ───────────
  # The main application. All roles are mounted on this single FastAPI server.
  # Endpoints:
  #   POST /ingest              — R1: Upload zip, detect artifact type
  #   POST /manifest/generate   — R3: Generate MCP manifest from source
  #   GET  /manifest/{id}       — R3: Retrieve generated manifest
  #   POST /mcp/{id}/start      — R2: Spawn MCP bridge subprocess
  #   GET  /mcp/{id}/health     — R2: Check bridge health
  #   POST /mcp/{id}/call       — R2: Proxy tool call to MCP server
  #   PATCH /mcp/{id}/status    — R2: Update bridge status
  #   POST /agent/link          — R4: Link agent to MCP server
  nasiko-server:
    build:
      context: ..
      dockerfile: Dockerfile
    ports:
      - "8000:8000"
    environment:
      # R5: Tell OpenTelemetry where to send traces
      - PHOENIX_COLLECTOR_ENDPOINT=http://phoenix-observability:6006/v1/traces
      - TRACING_ENABLED=true
      # R2: Kong admin URL for service registration
      - KONG_ADMIN_URL=http://kong:8001
      # R3: Source root for path validation
      - NASIKO_SOURCE_ROOT=/tmp/nasiko
      # R4: LLM gateway URL for env var injection
      - LLM_GATEWAY_URL=http://llm-gateway:4000
    depends_on:
      phoenix-observability:
        condition: service_healthy
      kong:
        condition: service_healthy
      llm-gateway:
        condition: service_started
    volumes:
      # Persist bridge state and manifests across restarts
      - nasiko_data:/tmp/nasiko
    networks:
      - nasiko_net

volumes:
  phoenix_data:
  nasiko_data:

networks:
  nasiko_net:
    driver: bridge
