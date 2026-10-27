# Failure Modes & Resilience Matrix

> **Analysis of system behavior, fault boundaries, and recovery procedures across all failure scenarios.**

---

## 1. Failure Matrix

| Component | Failure Scenario | Detection Point | Current System Behavior | User Impact | Recovery / Mitigation |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Configuration** | Missing `ANAKIN_API_KEY` | `server/anakin.py` (`search`) | Raises `AnakinConfigurationError`; `/api/scan` returns HTTP 500 with descriptive error detail. | Scan fails immediately. | Populate `ANAKIN_API_KEY` in `.env` and restart the backend server. |
| **Configuration** | Missing `DRONAHQ_WEBHOOK_URL` or `DRONAHQ_API_KEY` | `server/dronahq.py` (`trigger_reasoning`) | Raises `DronaHQConfigurationError`; `/api/scan` returns HTTP 500. | Scan fails before dispatching reasoning. | Configure `DRONAHQ_WEBHOOK_URL` and `DRONAHQ_API_KEY` in `.env`. |
| **Anakin Search** | Anakin API timeout (>20.0s) | `server/anakin.py` (`search`) | Raises `AnakinTimeoutError`; caught in `main.py`, returns HTTP 504 Gateway Timeout. | Scan fails; no research is collected. | Retry scan. If persistent, check Anakin network latency or reduce `ANAKIN_LIMIT`. |
| **Anakin Search** | Anakin returns HTTP 500 / 502 / 503 | `server/anakin.py` (`search`) | Raises `AnakinAPIError`; caught in `main.py`, returns HTTP 502 Bad Gateway. | Scan fails; user receives upstream status. | Retry request; verify Anakin API status. |
| **Anakin Search** | Anakin returns malformed JSON or missing `"results"` | `server/anakin.py` (`search`) | Raises `AnakinResponseError`; returns HTTP 502 Bad Gateway. | Scan fails cleanly without unhandled server crash. | Inspect Anakin response contract; verify API version. |
| **Anakin Search** | Anakin returns empty results (`results = []`) | `server/anakin.py` (`search`) | Successfully returns `[]`. DronaHQ is invoked with empty context. | Research stream is empty; DronaHQ triggers `insufficient_evidence`. | Broaden search prompt keywords in company products/segments. |
| **DronaHQ Reasoning** | Webhook timeout (>45.0s) | `server/dronahq.py` (`trigger_reasoning`) | Catches `TimeoutException`; sets `reasoning.status = "error"` with timeout message; `signals = []`. | **All 9 Anakin research sources are preserved and displayed to the user.** Signals are omitted. | Resilient fallback: User still gets full web research and source links. Retry scan for reasoning. |
| **DronaHQ Reasoning** | DronaHQ returns HTTP 500 / 502 | `server/dronahq.py` (`trigger_reasoning`) | Catches `DronaHQAPIError`; sets `reasoning.status = "error"`; `signals = []`. | **All 9 Anakin research sources are preserved.** Error message explains upstream reasoning failure. | User inspects raw research while DronaHQ service recovers. |
| **DronaHQ Reasoning** | Markdown code fences around JSON output | `server/dronahq.py` (`_clean_json_string`) | Strips ```` ```json ... ``` ```` backticks automatically before JSON parsing. | **Zero impact.** Normalization succeeds transparently. | Built-in de-fencing handles LLM markdown output variations. |
| **DronaHQ Reasoning** | Plain text / unparseable output | `server/dronahq.py` (`trigger_reasoning`) | Falls back to `{"status": "success", "signals": [raw_response]}`; extracts headline from first line; attaches sources. | Partial signal displayed with plain text summary; research fully preserved. | Logging captures raw output for prompt tuning. |
| **DronaHQ Reasoning** | Agent returns `status = "insufficient_evidence"` | `server/dronahq.py` (`trigger_reasoning`) | Sets `reasoning.status = "insufficient_evidence"`; returns `signals = []`. | Scan succeeds. UI displays collected research and notes lack of actionable competitive shifts. | **Zero hallucination.** Protects user from fictitious claims. |
| **DronaHQ Reasoning** | Alternate container key (`competitive_signals` vs `competitor_signals` vs `competitorSignals`) | `server/dronahq.py` (`trigger_reasoning`) | Evaluates all known keys in order of precedence. | **Zero impact.** Signals are extracted and normalized normally. | Built-in schema flexibility handles prompt variations. |
| **Client Request** | Invalid JSON body | FastAPI validation layer | Rejects with HTTP 422 Unprocessable Entity. | Scan does not run; error lists invalid fields. | Client corrects JSON payload syntax. |

---

## 2. Core Resilience Principle: Non-Destructive Failure

The most critical resilience guarantee in WARROOM is **Research Preservation**:
```python
except (dronahq.DronaHQTimeoutError, dronahq.DronaHQAPIError, dronahq.DronaHQResponseError) as exc:
    logger.warning("DronaHQ reasoning error, preserving collected research: %s", exc)
    dronahq_run = DronaHQRun(
        status="error",
        message=f"Research collected — AI reasoning unavailable: {exc}",
    )
    signals = []

return ScanResponse(
    company=company,
    research=all_research,  # <--- PRESERVED AND RETURNED
    reasoning=dronahq_run,
    signals=signals,
)
```

Because Anakin search succeeds in ~3–4 seconds while DronaHQ reasoning takes ~14–18 seconds, an upstream failure in the reasoning agent **never** invalidates the collected web evidence. The user still receives all 9 competitor news articles, URLs, and snippets.
