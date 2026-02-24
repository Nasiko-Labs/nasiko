FROM python:3.11-slim

WORKDIR /app

# Install uv
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/

# Copy application code
COPY . .

# Install dependencies directly without project setup
RUN uv pip install --system fastapi uvicorn motor pymongo pydantic

# Expose port
EXPOSE 8002

# Run the application
CMD ["python", "main.py"]