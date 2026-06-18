FROM python:3.12-slim
WORKDIR /app
COPY src/ /app
RUN pip install --no-cache-dir \
    "anthropic>=0.40.0" \
    "httpx>=0.27.0" \
    "a2a-sdk[http-server]==0.3.26" \
    "python-dotenv>=1.0.0" \
    "uvicorn>=0.30.0" \
    "click>=8.0.0"
ENV PYTHONUNBUFFERED=1
CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "8000"]
