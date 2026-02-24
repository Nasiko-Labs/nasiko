#!/bin/bash

# Navigate to agent directory
cd "$(dirname "$0")"

# Activate venv if it exists in project root or locally
PYTHON_CMD="python"
if [ -d "../../.venv" ]; then
    source ../../.venv/bin/activate
    PYTHON_CMD="../../.venv/bin/python"
elif [ -d ".venv" ]; then
    source .venv/bin/activate
    PYTHON_CMD=".venv/bin/python"
fi

echo "Using Python: $PYTHON_CMD"
$PYTHON_CMD --version

echo "Starting Agent with Tracing on Port 9100..."
# Add src to PYTHONPATH so that 'imports' inside src work if needed, 
# and also so we can import 'poc_obser' from project root.
export PYTHONPATH=$PYTHONPATH:../../
$PYTHON_CMD src/__main__.py --host 0.0.0.0 --port 9100
