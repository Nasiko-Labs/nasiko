import os
import json
import zipfile
import shutil

AGENTS = {
    "grant_scout": {
        "name": "Grant Scout",
        "description": "Searches live databases for active grants matching keywords",
        "entry": "main.py"
    },
    "compliance_analyst": {
        "name": "Compliance Analyst",
        "description": "Scrapes specific NOFO requirements and analyzes alignment",
        "entry": "main.py"
    },
    "synthesis": {
        "name": "Synthesis Agent",
        "description": "Merges findings into a unified grant match and strategy report using Groq 70B",
        "entry": "main.py"
    }
}

REQUIREMENTS = "fastapi\nuvicorn\nhttpx\ngroq\npython-dotenv\n"

DOCKERFILE = """FROM python:3.11-slim
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY . .
# Expose port 8000 which is the standard FastAPI port inside containers
EXPOSE 8000
CMD ["uvicorn", "main:app", "--host", "0.0.0.0", "--port", "8000"]
"""

def main():
    base_dir = os.path.dirname(__file__)
    packages_dir = os.path.join(base_dir, "packages")
    os.makedirs(packages_dir, exist_ok=True)
    
    for agent_dir, meta in AGENTS.items():
        agent_path = os.path.join(base_dir, agent_dir)
        if not os.path.exists(agent_path):
            print(f"Skipping {agent_dir}, folder not found.")
            continue
            
        # 1. Create AgentCard.json
        agent_card = {
            "name": meta["name"],
            "version": "1.0.0",
            "description": meta["description"],
            "entrypoint": meta["entry"]
        }
        with open(os.path.join(agent_path, "AgentCard.json"), "w") as f:
            json.dump(agent_card, f, indent=2)
            
        # 2. Create requirements.txt
        with open(os.path.join(agent_path, "requirements.txt"), "w") as f:
            f.write(REQUIREMENTS)
            
        # 3. Create Dockerfile
        with open(os.path.join(agent_path, "Dockerfile"), "w") as f:
            f.write(DOCKERFILE)
            
        # 4. Zip it up
        zip_path = os.path.join(packages_dir, f"{agent_dir}.zip")
        print(f"Packaging {agent_dir} -> {zip_path}")
        
        with zipfile.ZipFile(zip_path, 'w', zipfile.ZIP_DEFLATED) as zipf:
            for root, _, files in os.walk(agent_path):
                # Don't include pycache or other hidden folders
                if "__pycache__" in root:
                    continue
                for file in files:
                    file_path = os.path.join(root, file)
                    # Add to zip with relative path inside the folder
                    arcname = os.path.relpath(file_path, agent_path)
                    zipf.write(file_path, arcname)

    print("\nAll packages created successfully in the 'agents/packages' folder!")

if __name__ == "__main__":
    main()
