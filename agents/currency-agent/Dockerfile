FROM python:3.13-slim
WORKDIR /app
COPY pyproject.toml .
COPY src/ src/
RUN pip install --no-cache-dir .
ENV PYTHONUNBUFFERED=1
EXPOSE 8000
CMD ["python", "src/__main__.py", "--host", "0.0.0.0", "--port", "8000"]
