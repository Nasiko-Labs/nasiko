FROM python:3.12-slim

WORKDIR /app

COPY src/ /app

RUN pip install --no-cache-dir \
    "crewai[tools]>=0.95.0" \
    "a2a-sdk[http-server]>=0.3.0" \
    google-genai \
    pillow \
    pydantic \
    python-dotenv

ENV PYTHONUNBUFFERED=1

CMD ["python", "__main__.py", "--host", "0.0.0.0", "--port", "5000"]