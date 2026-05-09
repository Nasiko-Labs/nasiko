FROM python:3.12-slim

WORKDIR /app

# Install dependencies
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

# Copy source code
COPY firewall/ ./firewall/
COPY nasiko/ ./nasiko/
COPY events/ ./events/
COPY traces/ ./traces/
COPY tools/ ./tools/
COPY agents/ ./agents/
COPY config/ ./config/
COPY ui/ ./ui/
COPY main.py .
COPY AgentCard.json .

# Create sessions directory
RUN mkdir -p sessions

EXPOSE 8000

# Run the FastAPI service (Nasiko agent mode)
CMD ["uvicorn", "nasiko.api:app", "--host", "0.0.0.0", "--port", "8000"]
