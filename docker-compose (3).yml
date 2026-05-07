FROM python:3.11-slim
WORKDIR /app
COPY src/ src/
RUN pip install --no-cache-dir langchain langchain-openai langchain-core
CMD ["python", "src/main.py"]
