FROM python:3.12-slim
WORKDIR /app
COPY src/ /app
RUN pip install --no-cache-dir \
    "httpx>=0.27.0" \
    "a2a-sdk[http-server]==1.1.0" \
    "python-dotenv>=1.0.0" \
    "uvicorn>=0.30.0" \
    "click>=8.0.0" \
    "opentelemetry-sdk>=1.25.0" \
    "opentelemetry-distro>=0.46b0" \
    "opentelemetry-exporter-otlp>=1.25.0" \
    "opentelemetry-exporter-otlp-proto-grpc>=1.25.0" \
    "opentelemetry-instrumentation-httpx>=0.46b0" \
    "opentelemetry-instrumentation-starlette>=0.46b0"
ENV PYTHONUNBUFFERED=1
ENV OTEL_SERVICE_NAME=smart-llm-router-agent
# Nasiko control plane on the compose network
ENV ROUTER_BASE_URL=http://server:8080
    # Fan analytics out to the host dashboard (keep `python src/__main__.py` on :8000)
    # Set ANALYTICS_TOKEN on both sides in production.
    ENV ANALYTICS_INGEST_URL=http://host.docker.internal:8000/api/ingest
    CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "8000"]
