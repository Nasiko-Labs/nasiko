# Nasiko Metrics Cursor Agent Bridge

Server-side bridge for the Challenge 2 chatbot.

Keep `CURSOR_API_KEY` here on the server side. Do not put it in browser code.

## Setup

Docker:

```powershell
cd web\cursor-agent-bridge
$env:CURSOR_API_KEY="your_cursor_api_key_here"
docker compose up --build
```

Local Node:

```powershell
cd web\cursor-agent-bridge
npm install
$env:CURSOR_API_KEY="your_cursor_api_key_here"
npm start
```

The bridge runs at:

```text
http://127.0.0.1:8787/metrics-agent
```

The Challenge 2 dashboard checks this endpoint automatically. When the key is set and the bridge is running, the chatbot status changes to `Cursor bridge ready`.

Optional settings:

```powershell
$env:CURSOR_MODEL="composer-2"
$env:CURSOR_AGENT_CWD="D:\Nasiko\nasiko"
$env:PORT="8787"
```
