FROM python:3.12-slim
# Pi is invoked with --offline / PI_OFFLINE=1 so it does not git-clone or npm
# self-update at coder start. Do not add git here; the worker has no outbound
# package registry during a bounded Modal run.
RUN apt-get update && apt-get install -y --no-install-recommends nodejs npm \
    && npm install -g --ignore-scripts @earendil-works/pi-coding-agent \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY main.py telemetry.py AgentCard.json pi_coder.py ./
EXPOSE 8000
CMD ["python", "main.py"]
USER 65534:65534
