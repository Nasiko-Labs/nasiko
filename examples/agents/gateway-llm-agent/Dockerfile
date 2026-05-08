FROM python:3.11-slim

WORKDIR /app

COPY pyproject.toml .
RUN pip install --no-cache-dir \
    fastapi \
    uvicorn \
    openai \
    opentelemetry-sdk \
    opentelemetry-exporter-otlp-proto-http \
    openinference-instrumentation-openai \
    opentelemetry-instrumentation-httpx

COPY agent.py .

CMD ["uvicorn", "agent:app", "--host", "0.0.0.0", "--port", "8000"]
