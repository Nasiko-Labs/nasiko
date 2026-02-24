FROM python:3.11-slim

WORKDIR /app

COPY src/ /app

RUN pip install --no-cache-dir \
    "a2a-sdk[http-server]>=0.3.0" \
    click>=8.1.8 \
    openai>=1.57.0 \
    pydantic>=2.11.4 \
    python-dotenv>=1.1.0 \
    uvicorn>=0.34.2 \
    pandas>=2.0.0 \
    numpy>=1.24.0 \
    matplotlib>=3.7.0 \
    seaborn>=0.12.0 \
    plotly>=5.15.0 \
    kaleido>=0.2.1 \
    langchain-core>=0.2.0

ENV PYTHONUNBUFFERED=1

CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "5000"]