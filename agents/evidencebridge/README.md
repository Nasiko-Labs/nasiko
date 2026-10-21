# EvidenceBridge Agent

Supplier claim verification agent that researches public evidence and returns traceable verification findings before payment.

## Requirements

- `EVIDENCEBRIDGE_API_URL`: The URL of the EvidenceBridge backend API (e.g., `https://api.example.com/verify`).

## Execution

```sh
export EVIDENCEBRIDGE_API_URL="https://<evidencebridge-backend>/api/v1/agent/verify-supplier"
python main.py
```
