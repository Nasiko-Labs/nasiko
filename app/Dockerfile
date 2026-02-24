FROM python:3.12-slim

WORKDIR /app

# Install system dependencies including git
RUN apt-get update && apt-get install -y \
    git \
    && rm -rf /var/lib/apt/lists/*

# Install uv
COPY --from=ghcr.io/astral-sh/uv:latest /uv /bin/uv

# Copy dependency files
COPY pyproject.toml .
COPY uv.lock .

# Copy only the app directory and necessary files
COPY app/ ./app/
# Copy any additional files needed at runtime (if any)
# COPY README.md .

# Expose port
EXPOSE 8000

# Run the application (uv will handle dependency installation at runtime)
CMD ["uv", "run", "uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
