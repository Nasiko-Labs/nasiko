FROM python:3.13-slim

WORKDIR /app

COPY src/ /app

RUN pip install --no-cache-dir \
    "a2a-sdk[http-server]==1.1.0" \
    "click>=8.0.0" \
    "openai>=1.57.0" \
    "python-dotenv>=1.0.0" \
    "uvicorn>=0.30.0" \
    "requests>=2.31.0" \
    "beautifulsoup4>=4.12.0" \
    "langdetect>=1.0.9" \
    "opentelemetry-sdk>=1.25.0" \
    "opentelemetry-exporter-otlp-proto-grpc>=1.25.0" \
    "opentelemetry-instrumentation-httpx>=0.46b0" \
    "opentelemetry-instrumentation-starlette>=0.46b0" \
    "opentelemetry-instrumentation-openai>=0.35.0b0"

ENV PYTHONUNBUFFERED=1

EXPOSE 8000

CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "8000"]
