FROM python:3.11-slim

WORKDIR /app

# Install system dependencies for vector math
RUN apt-get update && apt-get install -y \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Install the specific libraries needed
# ADDED: sentence-transformers and openai
RUN pip install fastapi uvicorn redisvl httpx redis sentence-transformers openai

COPY request_layer.py .

EXPOSE 8090

CMD ["python", "request_layer.py"]