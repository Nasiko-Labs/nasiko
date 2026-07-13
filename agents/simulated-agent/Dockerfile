FROM python:3.13-slim
WORKDIR /app
COPY pyproject.toml .
RUN pip install --no-cache-dir .
COPY simulated-agent/main.py .
EXPOSE 8000
CMD ["python", "main.py", "--host", "0.0.0.0", "--port", "8000"]
