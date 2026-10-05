# Security Architecture & Practices

> **Security posture, credential protection, data boundary controls, and known limitations.**

---

## 1. Credential Protection & Secret Hygiene

### 1.1 Zero Secrets in Source Control
- All third-party secrets (`ANAKIN_API_KEY`, `DRONAHQ_API_KEY`, `DRONAHQ_WEBHOOK_URL`) reside strictly in local `.env` files.
- The repository `.gitignore` explicitly prevents `.env` and other secret artifacts from being checked into Git:
  ```gitignore
  .env
  .env.*
  !.env.example
  ```
- A sanitized [`.env.example`](file:///home/Krishna-Singh/WarRoom/.env.example) is committed with empty keys for onboarding.

### 1.2 No Frontend Exposure
- Neither `ANAKIN_API_KEY` nor `DRONAHQ_API_KEY` is embedded in the Vite client bundle or sent over client network requests.
- The client only communicates with the internal API gateway (`/api/scan`, `/api/health`).
- All outbound traffic to external providers is initiated strictly server-side in Python.

### 1.3 Logging Redaction
- Server logging configurations in [`server/main.py`](file:///home/Krishna-Singh/WarRoom/server/main.py), [`server/anakin.py`](file:///home/Krishna-Singh/WarRoom/server/anakin.py), and [`server/dronahq.py`](file:///home/Krishna-Singh/WarRoom/server/dronahq.py) format log messages to omit authorization headers, query strings containing tokens, and secret variables.

---

## 2. Input & Output Validation

- **Pydantic Validation**: All endpoints (`/api/scan`, `/api/company`) validate incoming JSON bodies against Pydantic schemas defined in [`server/schemas.py`](file:///home/Krishna-Singh/WarRoom/server/schemas.py).
- **Strict Typing**: Type mismatches, oversized payloads, or invalid structures trigger automatic HTTP 422 Unprocessable Entity responses.
- **Output Sanitization**: Normalized signal summaries and headlines are coerced to strings and validated before serialization in `ScanResponse`.

---

## 3. Network & CORS Policy

CORS middleware is explicitly configured in `server/main.py` to allow requests only from local frontend development servers:

```python
app.add_middleware(
    CORSMiddleware,
    allow_origins=[
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://localhost:3000",
        "http://127.0.0.1:3000",
    ],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)
```

Wildcard origins (`"*"`) are intentionally avoided in the CORS middleware configuration.

---

## 4. Evidence Integrity & Web Retrieval Safety

- Anakin search queries are executed over HTTPS (`https://api.anakin.io/v1/search`).
- DronaHQ webhook triggers are executed over HTTPS (`https://agents-backend.dronahq.com/webhook/...`).
- Source URLs returned by Anakin are verified web links to public competitor blogs, documentation, and news portals. The server does not execute arbitrary code or scripts from discovered web pages; it only reads text snippets.

---

## 5. Known Security Limitations (Hackathon Scope)

As an MVP buildathon implementation, the following controls are currently omitted and should be implemented prior to enterprise production deployment:

1. **Authentication & Authorization**: The `/api/scan` and `/api/company` endpoints do not require user authentication (JWT, OAuth2, or API session tokens). Anyone with network access to the server can trigger a scan.
2. **Rate Limiting**: There is currently no per-IP or per-user rate limiting. A client could issue concurrent `/api/scan` calls, potentially exhausting external API quotas.
3. **Transport Layer Security (TLS)**: Local development runs on unencrypted HTTP (`http://127.0.0.1:8000`). In staging/production, this must run behind a reverse proxy (Nginx, Caddy, or Cloudflare) with TLS termination.
4. **Data at Rest**: Company contexts in `data/company_context.json` are stored in plain JSON on the local filesystem.
