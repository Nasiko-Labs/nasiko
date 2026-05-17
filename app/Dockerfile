FROM python:3.12-slim

WORKDIR /app

# Install system dependencies including git
RUN apt-get update && apt-get install -y \
    git \
    && rm -rf /var/lib/apt/lists/*

# Install uv
COPY --from=ghcr.io/astral-sh/uv:latest /uv /bin/uv

# Copy dependency files and pre-install — baked into image layer so restarts are instant
COPY pyproject.toml .
COPY uv.lock .
RUN uv sync --no-install-project

# Copy app source
COPY app/ ./app/

# Expose port
EXPOSE 8000

CMD ["uv", "run", "uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
