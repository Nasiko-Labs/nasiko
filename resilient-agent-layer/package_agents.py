import os
import shutil
import json

base_dir = "/Users/keshav/AGENT-MS/resilient-agent-layer/agents"
agents = {
    "agent-a": {
        "file": "agent_a.py",
        "description": "Fast Agent - responds in 50-150ms",
        "capabilities": ["fast_processing"]
    },
    "agent-b": {
        "file": "agent_b.py",
        "description": "Medium Agent - responds in 300-700ms",
        "capabilities": ["medium_processing"]
    },
    "slow-agent": {
        "file": "slow_agent.py",
        "description": "Slow Agent - responds in 1.5-4s",
        "capabilities": ["heavy_processing"]
    }
}

for agent_name, agent_info in agents.items():
    agent_dir = os.path.join(base_dir, agent_name)
    os.makedirs(agent_dir, exist_ok=True)
    os.makedirs(os.path.join(agent_dir, "src"), exist_ok=True)
    
    # Move source file
    src_file = os.path.join(base_dir, agent_info["file"])
    dst_file = os.path.join(agent_dir, "src", "main.py")
    if os.path.exists(src_file):
        shutil.copy2(src_file, dst_file)
    
    # Create AgentCard.json
    agent_card = {
        "name": agent_name,
        "description": agent_info["description"],
        "capabilities": agent_info["capabilities"],
        "tags": ["demo", "resilient-layer"],
        "examples": ["test query"],
        "input_mode": "text",
        "output_mode": "json",
        "agent_protocol_version": "a2a-v1",
        "endpoints": {
            "/invoke": "Process query",
            "/health": "Health check"
        }
    }
    with open(os.path.join(agent_dir, "AgentCard.json"), "w") as f:
        json.dump(agent_card, f, indent=2)
        
    # Create pyproject.toml
    pyproject = f"""[project]
name = "{agent_name}"
version = "0.1.0"
description = "{agent_info['description']}"
dependencies = [
    "fastapi==0.115.5",
    "uvicorn[standard]==0.32.1",
    "pydantic==2.10.3"
]
"""
    with open(os.path.join(agent_dir, "pyproject.toml"), "w") as f:
        f.write(pyproject)
        
    # Create Dockerfile
    dockerfile = """FROM python:3.12-slim
WORKDIR /app
COPY pyproject.toml .
RUN pip install -e .
COPY src/ ./src/
EXPOSE 8000
CMD ["uvicorn", "src.main:app", "--host", "0.0.0.0", "--port", "8000"]
"""
    with open(os.path.join(agent_dir, "Dockerfile"), "w") as f:
        f.write(dockerfile)
        
    # Zip it up
    shutil.make_archive(os.path.join(base_dir, agent_name), 'zip', agent_dir)
    print(f"Created {agent_name}.zip")
