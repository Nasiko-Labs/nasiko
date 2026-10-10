# hackpay-git-eval (Nasiko agent)

Nasiko-native HackPay Git submission evaluator. Surfs public GitHub with **Anakin**, returns Accio-style scores, optional HackPay callback.

## Layout (Nasiko convention)

```
hackpay-git-eval/
  AgentCard.json   # name, skills, A2A metadata
  Dockerfile       # EXPOSE 8000
  server.js        # /evaluate + A2A JSON-RPC on /
  lib/             # anakin + scoring
  .env             # ANAKIN_API_KEY (not committed)
  deploy.ps1       # deploy helper
```

## Deploy to Nasiko

1. Fix Docker Desktop if hung, then start the control plane:

```powershell
nasiko up
# press Enter through the env prompt
```

2. Deploy this directory:

```powershell
cd E:\buildaws\nasiko\agents\hackpay-git-eval
.\deploy.ps1
# or:
nasiko connect http://localhost:8080
nasiko deploy . --name hackpay-git-eval --port 8000 --env-file .env -y -v 1.0.0
```

3. Wire HackPay `.env`:

```env
NASIKO_GIT_EVAL_URL=http://127.0.0.1:8000/evaluate
# after deploy, prefer the Nasiko proxy URL from `nasiko ps` / dashboard
```

## Local run (no Nasiko)

```powershell
cd E:\buildaws\nasiko\agents\hackpay-git-eval
node server.js
# listens on :8000
```

## Session history (sessions.html)

Every `/evaluate` (and A2A `message/send`) run is recorded into Nasiko
`chat_sessions` / `chat_messages` so it appears under
**Observability → Execution history** at http://localhost:8080/sessions.html.

Defaults (Docker compose network):

```env
NASIKO_SESSION_HISTORY=true
NASIKO_HISTORY_DATABASE_URL=postgres://nasiko:nasiko@postgres:5432/nasiko_dev
NASIKO_AGENT_NAME=hackpay-git-eval
NASIKO_CP_URL=http://server:8080
# optional instead of DB: NASIKO_API_TOKEN=<admin JWT>
```

## Endpoints

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Health + Anakin flag |
| GET | `/.well-known/agent.json` | AgentCard |
| POST | `/evaluate` | HackPay webhook body |
| POST | `/` | A2A JSON-RPC (`message/send`) for `nasiko chat` |
