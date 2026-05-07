FROM python:3.12-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY src/ ./src/

EXPOSE 8000

ENV GATEWAY_URL=http://litellm-gateway:4000
ENV LLM_MODEL=gemini-flash

CMD ["uvicorn", "src.main:app", "--host", "0.0.0.0", "--port", "8000"]
