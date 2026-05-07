FROM python:3.11-slim

WORKDIR /app

COPY src/ /app

RUN pip install --no-cache-dir \
    uvicorn>=0.34.0 \
    starlette>=0.41.0

ENV PYTHONUNBUFFERED=1

CMD ["python", "__main__.py"]
