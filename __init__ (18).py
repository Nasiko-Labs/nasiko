FROM python:3.11-slim
WORKDIR /app
COPY src/ src/
RUN pip install --no-cache-dir "mcp[cli]>=1.0"
CMD ["python", "src/main.py"]
