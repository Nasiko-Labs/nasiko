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
    pymongo>=4.0.0 \
    langchain-core \
    langchain-openai \
    PyPDF2 \
    openpyxl \
    pandas \
    python-docx

ENV PYTHONUNBUFFERED=1

CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "5000", "--mongo-url", "mongodb://agents-mongo:27017", "--db-name", "compliance-checker-a2a"]