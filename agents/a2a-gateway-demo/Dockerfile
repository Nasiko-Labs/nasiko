FROM python:3.11-slim

WORKDIR /app

COPY src/ /app

RUN pip install --no-cache-dir \
    "a2a-sdk[http-server]>=0.3.0" \
    click>=8.1.8 \
    openai>=1.57.0 \
    python-dotenv>=1.1.0 \
    uvicorn>=0.34.2 \
    opentelemetry-api>=1.25.0 \
    opentelemetry-sdk>=1.25.0

ENV PYTHONUNBUFFERED=1

CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "5001"]
