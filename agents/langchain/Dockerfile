FROM python:3.12-slim
WORKDIR /app
COPY src/ /app
RUN pip install --no-cache-dir \
    "langchain==0.2.17" \
    "langchain-core==0.2.43" \
    "langchain-openai==0.1.25" \
    "httpx==0.28.1" \
    "a2a-sdk[http-server]==0.3.26" \
    "python-dotenv==1.2.2" \
    "uvicorn==0.52.0" \
    "click==8.4.2" \
    "opentelemetry-api==1.44.0" \
    "opentelemetry-sdk==1.44.0" \
    "opentelemetry-distro==0.65b0" \
    "opentelemetry-exporter-otlp==1.44.0" \
    "opentelemetry-exporter-otlp-proto-grpc==1.44.0" \
    "opentelemetry-instrumentation-httpx==0.65b0" \
    "opentelemetry-instrumentation-starlette==0.65b0" \
    "opentelemetry-instrumentation-openai-v2==2.4b0" \
    "opentelemetry-instrumentation-openai==0.62.1" \
    "opentelemetry-instrumentation-langchain==0.62.1"
ENV PYTHONUNBUFFERED=1
CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "8000"]
