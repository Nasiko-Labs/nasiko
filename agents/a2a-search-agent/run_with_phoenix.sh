#!/bin/bash

# Ensure we run from the agent directory
cd "$(dirname "$0")" || exit 1

# Set environment variables for observability
export PHOENIX_PROJECT_NAME="a2a-search-agent"
export PHOENIX_API_KEY="${PHOENIX_API_KEY:-your_phoenix_api_key_here}"

# Check if OPENAI_API_KEY is set
if [ -z "$OPENAI_API_KEY" ]; then
    echo "Error: OPENAI_API_KEY environment variable is not set"
    echo "Please set it with: export OPENAI_API_KEY='your-openai-api-key'"
    exit 1
fi

# Run the search agent
echo "Starting A2A Search Agent with Phoenix observability..."
echo "Agent will be available at http://localhost:5000"
echo "Press Ctrl+C to stop"

python -m src --host localhost --port 5000