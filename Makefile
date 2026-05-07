FROM python:3.11-slim

WORKDIR /app

# System deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc \
    && rm -rf /var/lib/apt/lists/*

# Copy dependency definition first (Docker layer caching)
COPY pyproject.toml .

# Install Python dependencies
RUN pip install --no-cache-dir \
    "pydantic>=2.0" \
    "httpx>=0.24" \
    "fastapi>=0.100" \
    "uvicorn>=0.23" \
    "tenacity>=8.0" \
    "python-multipart>=0.0.6" \
    "opentelemetry-api>=1.36.0" \
    "opentelemetry-sdk>=1.36.0" \
    "opentelemetry-exporter-otlp>=1.36.0" \
    "opentelemetry-instrumentation-fastapi>=0.48b0" \
    "arize-phoenix>=12.0.0" \
    "redis"

# Copy application code
COPY nasiko/ nasiko/

# Create /tmp/nasiko directories the app expects
RUN mkdir -p /tmp/nasiko/uploads

# Expose bridge port
EXPOSE 8000

# Health check
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD python -c "import httpx; httpx.get('http://localhost:8000/docs')" || exit 1

# Run the bridge server
CMD ["uvicorn", "nasiko.mcp_bridge.server:app", "--host", "0.0.0.0", "--port", "8000"]
