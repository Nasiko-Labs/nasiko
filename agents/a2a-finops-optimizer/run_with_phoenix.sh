#!/bin/bash

# Script to run the FinOps Optimizer agent with Phoenix tracing

# Check if OPENAI_API_KEY is set
if [ -z "$OPENAI_API_KEY" ]; then
    echo "Error: OPENAI_API_KEY environment variable is not set"
    echo "Please set it with: export OPENAI_API_KEY='your-api-key'"
    exit 1
fi

# Start Phoenix in the background if not already running
if ! pgrep -f "phoenix" > /dev/null; then
    echo "Starting Phoenix tracing server..."
    python -m phoenix.server.main serve &
    PHOENIX_PID=$!
    echo "Phoenix started with PID: $PHOENIX_PID"
    sleep 5
fi

# Run the agent
echo "Starting FinOps Optimizer Agent..."
python -m src --host localhost --port 5000

# Cleanup Phoenix if we started it
if [ ! -z "$PHOENIX_PID" ]; then
    echo "Stopping Phoenix..."
    kill $PHOENIX_PID
fi