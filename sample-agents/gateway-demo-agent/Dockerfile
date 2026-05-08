FROM python:3.12-slim
WORKDIR /app
RUN pip install openai
COPY src/ ./src/
CMD ["python", "src/main.py"]
