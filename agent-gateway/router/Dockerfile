# Python app Dockerfile
FROM python:3.12-slim

WORKDIR /app

# Install system dependencies
# RUN apt-get update && apt-get install -y \
#     curl \
#     ca-certificates \
#     unzip \
#     && rm -rf /var/lib/apt/lists/*

# Copy Python project files
COPY . /app/router

# Install Python dependencies using Poetry
RUN pip install --upgrade pip
RUN cd /app/router && pip install poetry && poetry config virtualenvs.create false && poetry install --no-interaction --no-ansi

# Expose any port your Python app uses, e.g., FastAPI
EXPOSE 8000

# Command to run your Python app
CMD ["uvicorn", "router.src.main:app", "--host", "0.0.0.0", "--port", "8000", "--reload"]
