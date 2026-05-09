FROM python:3.11-slim

WORKDIR /app

RUN apt-get update && apt-get install -y curl && rm -rf /var/lib/apt/lists/*

COPY pyproject.toml .
RUN pip install --no-cache-dir \
    fastapi \
    "uvicorn[standard]" \
    httpx \
    "redis>=6.4.0" \
    pydantic \
    "pydantic-settings>=2.0.0" \
    python-json-logger \
    opentelemetry-sdk \
    opentelemetry-exporter-otlp-proto-grpc

COPY src/ ./src/

EXPOSE 8090

CMD ["uvicorn", "src.main:app", "--host", "0.0.0.0", "--port", "8090"]
