FROM python:3.12-slim

WORKDIR /app

COPY src/ /app

RUN pip install --no-cache-dir \
    click>=8.1.8 \
    httpx>=0.28.1 \
    langgraph>=0.3.18 \
    langchain-openai>=0.1.0 \
    langchain-core \
    pydantic>=2.10.6 \
    python-dotenv>=1.1.0 \
    uvicorn>=0.34.2 \
    "a2a-sdk[http-server]==0.3.26" \
    "opentelemetry-sdk>=1.25.0" \
    "opentelemetry-distro>=0.46b0" \
    "opentelemetry-exporter-otlp>=1.25.0" \
    "opentelemetry-exporter-otlp-proto-grpc>=1.25.0" \
    "opentelemetry-instrumentation-httpx>=0.46b0" \
    "opentelemetry-instrumentation-starlette>=0.46b0" \
    "opentelemetry-instrumentation-openai-v2>=2.0b0" \
    "opentelemetry-instrumentation-openai>=0.35.0b0" \
    "opentelemetry-instrumentation-langchain>=0.35.0b0"

ENV PYTHONUNBUFFERED=1

CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "8000"]
