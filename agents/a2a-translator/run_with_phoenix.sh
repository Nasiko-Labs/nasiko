#!/bin/bash

# Set environment variables for observability
export PHOENIX_PROJECT_NAME="a2a-translator"
export PHOENIX_API_KEY="your_phoenix_api_key_here"

# Check if OPENAI_API_KEY is set
if [ -z "$OPENAI_API_KEY" ]; then
    echo "Error: OPENAI_API_KEY environment variable is not set"
    echo "Please set it with: export OPENAI_API_KEY='your-openai-api-key'"
    exit 1
fi

# Run the translator agent
echo "Starting A2A Translator Agent with Phoenix observability..."
echo "Agent will be available at http://localhost:5000"
echo "Press Ctrl+C to stop"

python -m src --host localhost --port 5000