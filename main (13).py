services:
  mcp-calculator:
    build: .
    container_name: mcp-calculator
    stdin_open: true
    ports:
      - "5000"
    tty: true
    environment:
      - TRACING_ENABLED=true
      - PHOENIX_COLLECTOR_ENDPOINT=http://phoenix-observability:4318/v1/traces
    networks:
      - agents-net

networks:
  agents-net:
    external: true
