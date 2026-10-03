FROM python:3.11-slim
WORKDIR /service
COPY classifier_service/requirements.txt ./requirements.txt
RUN pip install --no-cache-dir -r requirements.txt
COPY classifier_service/app.py ./app.py
EXPOSE 8001
CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "8001"]
