FROM python:3.13-slim

WORKDIR /app

COPY src/ /app

RUN pip install --no-cache-dir \
    "a2a-sdk[http-server]==1.1.1" \
    "click>=8.0.0" \
    "openai>=1.57.0" \
    "python-dotenv>=1.0.0" \
    "uvicorn>=0.30.0" \
    "opentelemetry-distro>=0.46b0" \
    "opentelemetry-exporter-otlp>=1.25.0" \
    "opentelemetry-instrumentation-openai-v2>=2.0b0"

ENV PYTHONUNBUFFERED=1

EXPOSE 8000

CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "8000"]