#!/bin/bash
set -e

# Start the server in the background
ollama serve &

# PID of the server
SERVER_PID=$!

# Wait until the server is responding (poll port)
until echo > /dev/tcp/localhost/11434; do
    sleep 1
done

echo "Server is ready"

# Create the model
ollama create arch-function -f /root/.ollama/model/Modelfile

echo "Model created."

# Bring server to foreground
wait $SERVER_PID
