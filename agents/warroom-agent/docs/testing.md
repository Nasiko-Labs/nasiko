# Testing Strategy & Verification

> **Automated test suite, mocking architecture, and live verification procedures.**

---

## 1. Overview of Test Strategy

WARROOM employs a dual-layer verification strategy:
1. **Automated Unit & Integration Tests (Mocked)**: Fast, deterministic tests that run in milliseconds using `httpx.MockTransport` and `ASGITransport` without consuming external API credits or requiring network connectivity.
2. **Live Integration Verification (Real APIs)**: Periodic end-to-end scans testing live communication with the Anakin.io Search API and the published DronaHQ reasoning webhook.

---

## 2. Automated Test Suite (11 Tests)

All automated tests are located under [`tests/`](file:///home/Krishna-Singh/WarRoom/tests) and execute via `pytest` with `pytest-asyncio`.

### 2.1 Test File Breakdown

| Test File | Target Module | Coverage Summary |
| :--- | :--- | :--- |
| [`tests/test_health.py`](file:///home/Krishna-Singh/WarRoom/tests/test_health.py) | `server/main.py` | Validates `GET /api/health` returns `200 OK` and `{status: "ok", service: "warroom-api"}` without leaking environment variables. |
| [`tests/test_anakin.py`](file:///home/Krishna-Singh/WarRoom/tests/test_anakin.py) | `server/anakin.py` | 1. Validates outgoing request payload structure (`prompt`, `limit`) and header (`X-API-Key`).<br>2. Verifies parsing of search results, snippet extraction, and date handling.<br>3. Verifies `AnakinConfigurationError` is raised if `ANAKIN_API_KEY` is missing. |
| [`tests/test_dronahq.py`](file:///home/Krishna-Singh/WarRoom/tests/test_dronahq.py) | `server/dronahq.py` | 1. **JSON String Response**: Tests parsing when `response` is a stringified JSON object.<br>2. **Parsed Object Response**: Tests parsing when `response` is an already-decoded dict.<br>3. **Markdown-Fenced Response**: Tests de-fencing when `response` is wrapped in ```` ```json ... ``` ```` containing `competitive_signals`.<br>4. **Insufficient Evidence**: Tests agent returning `status: "insufficient_evidence"` yielding empty signals without hallucinations.<br>5. **Malformed Plain Text**: Tests recovery when agent returns unformatted text, extracting a clean fallback signal without crashing.<br>6. **HTTP Failure**: Tests `DronaHQAPIError` is raised on HTTP 500 error status. |
| [`tests/test_scan.py`](file:///home/Krishna-Singh/WarRoom/tests/test_scan.py) | `server/main.py` | 1. **Full Scan Pipeline**: Simulates Anakin search and DronaHQ reasoning, verifying default PayFlow context, 3 competitors queried, prompt composition, and signal wiring into `ScanResponse`.<br>2. **Research Preservation**: Verifies that if DronaHQ encounters a timeout or error, all collected Anakin search results are safely preserved and returned with `reasoning.status = "error"`. |

---

## 3. Running Automated Tests

```bash
# Activate virtual environment
source .venv/bin/activate

# Execute pytest with verbose output
pytest -v
```

### Expected Output
```text
============================= test session starts ==============================
platform linux -- Python 3.14.7, pytest-9.1.1, pluggy-1.6.0
rootdir: /home/Krishna-Singh/WarRoom
configfile: pytest.ini
plugins: asyncio-1.4.0, anyio-4.15.1
asyncio: mode=Mode.AUTO

tests/test_anakin.py::test_anakin_search_request_format_and_parsing PASSED [  9%]
tests/test_anakin.py::test_anakin_search_missing_api_key_raises_configuration_error PASSED [ 18%]
tests/test_dronahq.py::test_dronahq_response_with_json_string PASSED     [ 27%]
tests/test_dronahq.py::test_dronahq_response_with_parsed_object PASSED   [ 36%]
tests/test_dronahq.py::test_dronahq_insufficient_evidence PASSED         [ 45%]
tests/test_dronahq.py::test_dronahq_response_with_competitive_signals_markdown PASSED [ 54%]
tests/test_dronahq.py::test_dronahq_malformed_response PASSED            [ 63%]
tests/test_dronahq.py::test_dronahq_http_error PASSED                    [ 72%]
tests/test_health.py::test_health_check_returns_ok PASSED                [ 81%]
tests/test_scan.py::test_scan_endpoint_processes_competitors_and_wires_signals PASSED [ 90%]
tests/test_scan.py::test_scan_endpoint_preserves_research_on_dronahq_failure PASSED [100%]

============================== 11 passed in 0.30s ==============================
```

---

## 4. Live Integration Verification Procedure

Live verification is conducted against production endpoints using real credentials configured in `.env`.

### Step 1: Start Backend Server
```bash
source .venv/bin/activate
uvicorn server.main:app --reload --port 8000
```

### Step 2: Dispatch Live Scan
```bash
curl -s -X POST http://127.0.0.1:8000/api/scan \
  -H "Content-Type: application/json" \
  -d '{}' > /tmp/live_scan.json
```

### Step 3: Inspect Output
```bash
python3 -c "
import json
with open('/tmp/live_scan.json') as f:
    d = json.load(f)

print('Company:', d['company']['name'])
print('Research count:', {r['competitor']: len(r['results']) for r in d['research']})
print('Total sources:', sum(len(r['results']) for r in d['research']))
print('Reasoning status:', d['reasoning']['status'])
print('Signals detected:', len(d['signals']))
for s in d['signals']:
    print(f\"- [{s['competitor']}] {s['headline']}\")
"
```

### Verified Live Results
- **Company**: PayFlow
- **Anakin Sources Collected**: 9 total (3 Cashfree, 3 Razorpay, 3 PayU)
- **DronaHQ HTTP Status**: 200 OK
- **Reasoning Status**: `completed`
- **Signals Normalized**: 3 verified signals
  - **Cashfree**: Launch of 'RiskShield', a real-time fraud prevention solution.
  - **Razorpay**: Introduction of Chargeback Shield, leveraging ML for fraud detection.
  - **PayU**: Enhanced anti-fraud solution utilizing machine learning.
