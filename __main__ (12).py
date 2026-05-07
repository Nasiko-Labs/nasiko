FROM python:3.11-slim

WORKDIR /app

COPY src/ /app

# Core: starlette + uvicorn for HTTP server
# Optional: opentelemetry for Phoenix tracing (pure Python, no numpy)
RUN pip install --no-cache-dir \
    uvicorn>=0.34.0 \
    starlette>=0.41.0 \
    opentelemetry-api>=1.20.0 \
    opentelemetry-sdk>=1.20.0 \
    opentelemetry-exporter-otlp-proto-http>=1.20.0

ENV PYTHONUNBUFFERED=1
ENV TRACING_ENABLED=true
ENV PHOENIX_COLLECTOR_ENDPOINT=http://phoenix-observability:6006/v1/traces

CMD ["python", "main.py"]
