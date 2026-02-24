#!/bin/bash

# AgentCard Generator CLI Runner
#
# Configure these variables before running:

# Agent path to analyze (required)
AGENT_PATH="${1:-../../../agents/a2a-compliance-checker}"
AGENT_PATH="${1:-../../../agents/a2a-compliance-checker}"

# Optional settings
VERBOSE=true          # Set to true for detailed output
MODEL="gpt-4o"        # Model to use
API_KEY=""
N8N_AGENT=true

# Build command arguments
ARGS="$AGENT_PATH"

if [ "$VERBOSE" = true ]; then
    ARGS="$ARGS -v"
fi

if [ -n "$MODEL" ]; then
    ARGS="$ARGS --model $MODEL"
fi

if [ -n "$API_KEY" ]; then
    ARGS="$ARGS --api-key $API_KEY"
fi

# if [ "$N8N_AGENT" = true ]; then
#     ARGS="$ARGS --n8n-agent "
# fi
# if [ "$N8N_AGENT" = true ]; then
#     ARGS="$ARGS --n8n-agent "
# fi

if [ -n "$OUTPUT_PATH" ]; then
    ARGS="$ARGS -o $OUTPUT_PATH"
fi


echo "Generating AgentCard for: $AGENT_PATH"
echo "Model: $MODEL"
echo ""

# Run the CLI
../../../.venv/bin/python -m cli $ARGS