FROM python:3.11-slim

WORKDIR /app

COPY src/ /app

RUN pip install --no-cache-dir \
    "fastapi>=0.109.0" \
    "uvicorn>=0.27.0" \
    "click>=8.1.7" \
    "langchain>=0.2.0,<0.3.0" \
    "langchain-openai>=0.1.0,<0.2.0" \
    "openai>=1.0.0" \
    "pydantic>=2.6.0" \
    "beautifulsoup4" \
    "requests>=2.31.0" \
    "langchain-community"

ENV PYTHONUNBUFFERED=1

CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "5000"]
