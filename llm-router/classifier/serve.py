"""HTTP classifier backend for llm-router (REQUEST_CLASSIFIER_BACKEND=http).

    uvicorn serve:app --port 8099
    REQUEST_CLASSIFIER_ENDPOINT=http://127.0.0.1:8099/classify

Request:  {"query": str, "context": str|null}
Response: {"request_type", "complexity" (1-5), "confidence" (0-1, temperature-calibrated)}
Deterministic: no sampling. The router falls back to regex when confidence < its configured minimum.
"""

from pathlib import Path

import joblib
import numpy as np
from fastapi import FastAPI
from pydantic import BaseModel
from scipy.special import softmax

from model import featurize

bundle = joblib.load(Path(__file__).parent / "model.joblib")
clf, reg, T, LABELS = bundle["clf"], bundle["reg"], bundle["temperature"], bundle["labels"]
featurize(["warm up"], [""])  # load the encoder once, before the first request

app = FastAPI()


class Req(BaseModel):
    query: str
    context: str | None = None


@app.get("/health")
def health():
    return {"ok": True, "recommended_min_confidence": bundle["threshold"]}


@app.post("/classify")
def classify(req: Req):
    x = featurize([req.query], [req.context or ""])
    p = softmax(clf.decision_function(x)[0] / T)
    i = int(np.argmax(p))
    complexity = int(np.clip(np.rint(reg.predict(x)[0]), 1, 5))
    return {"request_type": LABELS[i], "complexity": complexity, "confidence": round(float(p[i]), 4)}
