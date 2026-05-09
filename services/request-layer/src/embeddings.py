import logging
import numpy as np
from sentence_transformers import SentenceTransformer

logger = logging.getLogger(__name__)

_MODEL_NAME = "all-MiniLM-L6-v2"
_model: SentenceTransformer | None = None


def load_model() -> None:
    global _model
    logger.info(f"loading embedding model: {_MODEL_NAME}")
    _model = SentenceTransformer(_MODEL_NAME)
    _model.encode("warmup", convert_to_numpy=True)
    logger.info("embedding model ready")


def get_embedding(text: str) -> np.ndarray:
    if _model is None:
        raise RuntimeError("embedding model not loaded — call load_model() at startup")
    return _model.encode(text, convert_to_numpy=True, normalize_embeddings=True)


def cosine_similarity(a: np.ndarray, b: np.ndarray) -> float:
    return float(np.dot(a, b))
