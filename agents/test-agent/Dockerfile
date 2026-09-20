FROM python:3.12-slim

WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY src/ ./src/
COPY AgentCard.json .

ENV PORT=8000
EXPOSE $PORT

CMD ["python", "src/main.py"]
