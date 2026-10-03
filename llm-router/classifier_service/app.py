"""Private local inference adapter for the exported LLMRouter KNN classifier."""

from __future__ import annotations

import os
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any

import joblib
import numpy as np
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

REQUEST_TYPES = (
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
)
MAX_QUERY_CHARS = 12_000


class LongformerEncoder:
    """Pinned Longformer mean-pooling path used to build the LLMRouter KNN vectors."""

    def __init__(self, revision: str):
        import torch
        from transformers import AutoModel, AutoTokenizer

        self.torch = torch
        self.tokenizer = AutoTokenizer.from_pretrained(
            "allenai/longformer-base-4096", revision=revision
        )
        self.model = AutoModel.from_pretrained(
            "allenai/longformer-base-4096", revision=revision
        )
        self.model.eval()

    def __call__(self, text: str) -> np.ndarray:
        inputs = self.tokenizer(
            text,
            return_tensors="pt",
            truncation=True,
            max_length=4096,
        )
        with self.torch.inference_mode():
            hidden = self.model(**inputs).last_hidden_state
            mask = inputs["attention_mask"].unsqueeze(-1).to(hidden.dtype)
            pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1).clamp(min=1)
        return pooled.cpu().numpy().astype(np.float32)


def classify_with_bundle(text: str, bundle: dict[str, Any], embed) -> dict[str, Any]:
    vector = embed(text)
    knn = bundle["request_type_knn"]
    distances, indices = knn.kneighbors(vector, n_neighbors=knn.n_neighbors)
    labels = bundle["request_type_labels"]
    weights: dict[str, float] = {label: 0.0 for label in REQUEST_TYPES}
    for distance, index in zip(distances[0], indices[0], strict=True):
        label = str(labels[index])
        weight = 1.0 / max(float(distance), 1e-8) if knn.weights == "distance" else 1.0
        weights[label] += weight
    # Explicit category order makes equal-vote results stable across Python versions.
    request_type = max(REQUEST_TYPES, key=lambda label: weights[label])
    total_weight = sum(weights.values())
    raw_confidence = weights[request_type] / total_weight if total_weight else 0.0
    calibrated = float(bundle["confidence_calibrator"].predict([raw_confidence])[0])

    complexity_model = bundle["complexity_knn"]
    complexity_distances, complexity_indices = complexity_model.kneighbors(
        vector, n_neighbors=complexity_model.n_neighbors
    )
    complexity_values = bundle["complexity_labels"]
    complexity_weights = [
        1.0 / max(float(distance), 1e-8)
        if complexity_model.weights == "distance"
        else 1.0
        for distance in complexity_distances[0]
    ]
    complexity = sum(
        float(complexity_values[index]) * weight
        for index, weight in zip(complexity_indices[0], complexity_weights, strict=True)
    ) / sum(complexity_weights)

    return {
        "request_type": request_type,
        "complexity": max(1, min(5, int(np.floor(complexity + 0.5)))),
        "confidence": max(0.0, min(1.0, calibrated)),
    }


class ClassifyRequest(BaseModel):
    query: str = Field(min_length=1, max_length=MAX_QUERY_CHARS)
    context: str | None = Field(default=None, max_length=MAX_QUERY_CHARS)


@asynccontextmanager
async def lifespan(app: FastAPI):
    artifact = Path(os.environ.get("CLASSIFIER_MODEL_PATH", "/models/request_classifier.joblib"))
    if not artifact.is_file():
        raise RuntimeError(f"classifier artifact is missing: {artifact}")
    bundle = joblib.load(artifact)
    if bundle.get("schema_version") != 1:
        raise RuntimeError("unsupported classifier artifact schema")
    revision = bundle.get("metadata", {}).get("embedding_revision")
    if not revision or revision == "main":
        raise RuntimeError("artifact must contain a resolved Longformer commit revision")
    encoder = LongformerEncoder(revision)
    encoder("warmup")
    app.state.classifier_bundle = bundle
    app.state.encoder = encoder
    yield


app = FastAPI(title="Nasiko request classifier", docs_url=None, redoc_url=None, lifespan=lifespan)


@app.get("/health")
def health() -> dict[str, str]:
    ready = hasattr(app.state, "classifier_bundle")
    return {"status": "ok" if ready else "starting"}


@app.post("/classify")
def classify(request: ClassifyRequest) -> dict[str, Any]:
    text = request.query
    if request.context:
        text = f"{text}\nContext:\n{request.context}"
    try:
        return classify_with_bundle(text, app.state.classifier_bundle, app.state.encoder)
    except Exception as error:
        # Do not include input text or exception details in responses/logs.
        raise HTTPException(status_code=503, detail="classifier inference failed") from error
