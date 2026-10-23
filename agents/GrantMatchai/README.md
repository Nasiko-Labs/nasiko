# GrantMatch AI Setup Instructions

## 1. Environment Variables
Before running the agents, you must add your API keys to `d:\Projects\GrantMach AI\agents\.env`:
```
GROQ_API_KEY=your_groq_key_here
ANAKIN_API_KEY=your_anakin_key_here
```

## 2. Starting the Agents
Open a terminal in the `d:\Projects\GrantMach AI` directory and activate the virtual environment, then run the agents script:
```powershell
.\.venv\Scripts\Activate.ps1
cd agents
python run_agents.py
```
This will start:
- Grant Scout on port 8001
- Compliance Analyst on port 8002
- Synthesis on port 8003

*Note: If your Nasiko orchestrator is running in Docker and needs to communicate with these agents, use `http://host.docker.internal:8001` (and so on) as the agent URLs.*

## 3. Tunneling Nasiko for DronaHQ
Since DronaHQ is a cloud service, it needs a public URL to talk to your local Nasiko instance (running on `localhost:8080`). 

1. Install [Ngrok](https://ngrok.com/download) if you haven't already.
2. In a new terminal, run:
```powershell
ngrok http 8080
```
3. Copy the `Forwarding` URL (e.g., `https://abc-123.ngrok-free.app`).
4. Use this URL in DronaHQ to configure your REST API queries (e.g., POST to `https://abc-123.ngrok-free.app/api/v1/tasks`).
