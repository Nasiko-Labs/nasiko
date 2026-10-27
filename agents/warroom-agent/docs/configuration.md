# Configuration & Environment

> **Environment variables, configuration schema, and secret hygiene guidelines.**

---

## 1. Environment Variables Overview

WARROOM utilizes a centralized configuration loader implemented in [`server/config.py`](file:///home/Krishna-Singh/WarRoom/server/config.py) using `python-dotenv`. Settings are loaded once at startup from the local `.env` file or from host environment variables.

| Variable | Type | Required | Default | Purpose |
| :--- | :--- | :--- | :--- | :--- |
| `ANAKIN_API_KEY` | String | **Yes** | None | API key for authenticating with Anakin.io Search API |
| `DRONAHQ_WEBHOOK_URL` | String | **Yes** | None | Full HTTPS webhook URL for the published DronaHQ agent |
| `DRONAHQ_API_KEY` | String | **Yes** | None | API key sent in the `api-key` header to DronaHQ |
| `ANAKIN_LIMIT` | Integer | No | `3` | Number of live search results requested per competitor |
| `PORT` | Integer | No | `8000` | Port for the FastAPI Uvicorn server |

---

## 2. `.env.example` Template

A sanitized configuration template is tracked in the repository root at [`.env.example`](file:///home/Krishna-Singh/WarRoom/.env.example):

```ini
ANAKIN_API_KEY=
DRONAHQ_WEBHOOK_URL=
DRONAHQ_API_KEY=
ANAKIN_LIMIT=3
PORT=8000
```

To initialize your local environment:
```bash
cp .env.example .env
```

Edit `.env` and insert your actual API keys.

---

## 3. Configuration Loading Mechanism

The configuration is encapsulated in the `Settings` class in [`server/config.py`](file:///home/Krishna-Singh/WarRoom/server/config.py):

```python
class Settings:
    anakin_api_key: str | None
    dronahq_webhook_url: str | None
    dronahq_api_key: str | None
    anakin_limit: int
    port: int

    def __init__(self):
        self.reload()

    def reload(self):
        self.anakin_api_key = os.getenv("ANAKIN_API_KEY")
        self.dronahq_webhook_url = os.getenv("DRONAHQ_WEBHOOK_URL")
        self.dronahq_api_key = os.getenv("DRONAHQ_API_KEY")
        # ...
```

### Verification Methods:
- `settings.is_anakin_configured()`: Validates that `ANAKIN_API_KEY` is present and non-empty.
- `settings.is_dronahq_configured()`: Validates that both `DRONAHQ_WEBHOOK_URL` and `DRONAHQ_API_KEY` are present and non-empty.

---

## 4. Secret Hygiene Rules

To protect API credentials from accidental leakage:
1. **Never Commit `.env`**: `.gitignore` explicitly excludes `.env`, `.env.local`, and related credential files.
2. **Never Log Secret Keys**: Server logging in `server/main.py`, `server/anakin.py`, and `server/dronahq.py` excludes sensitive authorization headers and raw keys.
3. **No Frontend Exposure**: Neither the Vite client code nor the client API responses expose `ANAKIN_API_KEY` or `DRONAHQ_API_KEY`. All communication with external providers is mediated server-side.
4. **Health Check Sanitization**: `GET /api/health` reports service liveness without printing configuration values or credential statuses.
