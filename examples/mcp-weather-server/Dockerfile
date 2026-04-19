FROM python:3.11-slim

WORKDIR /app

# Copy source
COPY src/ ./src/
COPY pyproject.toml ./

# Install dependencies
RUN pip install --no-cache-dir -e .

# Run the MCP server
CMD ["python", "src/main.py"]
