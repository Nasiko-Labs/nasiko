"""Embedding model singleton."""
import asyncio
import logging
import threading
from typing import Sequence

logger = logging.getLogger(__name__)


_lock = threading.Lock()
_model = None  # type: ignore[var-annotated]


def load_model(model_name: str) -> None:
    """Load the embedding model into the module-level singleton.

    Safe to call multiple times; subsequent calls are no-ops if the model is
    already loaded with the same name.
    """

    global _model
    if _model is not None:
        return
    with _lock:
        if _model is not None:
            return
        logger.info("loading embedding model: %s", model_name)
        # Imported lazily so unit tests that don't need embeddings can avoid
        # paying the ~500ms import cost.
        from sentence_transformers import SentenceTransformer

        _model = SentenceTransformer(model_name)
        logger.info("embedding model ready (dim=%s)", _model.get_sentence_embedding_dimension())


def is_loaded() -> bool:
    """Return ``True`` if the model has been loaded."""

    return _model is not None


def _embed_sync(texts: Sequence[str]) -> list[list[float]]:
    if _model is None:
        raise RuntimeError("embedding model is not loaded; call load_model first")
    vectors = _model.encode(
        list(texts),
        normalize_embeddings=True,
        convert_to_numpy=True,
        show_progress_bar=False,
    )
    return [v.tolist() for v in vectors]


async def embed(texts: Sequence[str]) -> list[list[float]]:
    """Encode ``texts`` and return one vector per input.

    Runs the model in the default thread pool so the event loop is not
    blocked. Returns L2-normalized vectors so cosine similarity reduces to a
    dot product (matches Redis HNSW ``DISTANCE_METRIC COSINE`` semantics).
    """

    if not texts:
        return []
    return await asyncio.to_thread(_embed_sync, texts)


async def embed_one(text: str) -> list[float]:
    """Convenience wrapper for the single-text case."""

    vectors = await embed([text])
    return vectors[0]
