"""
Refactored main router application with modular architecture.
"""

import logging
from io import BytesIO
from typing import List, Optional

from fastapi import FastAPI, File, Form, UploadFile, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.params import Depends
from fastapi.responses import StreamingResponse
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

from router.src.config import settings
from router.src.entities import UserRequest
from router.src.services import RouterOrchestrator
from router.src.core.resilient_executor import get_cache, get_limiter, get_stats

# Configure logging
logging.basicConfig(
    level=getattr(logging, settings.LOG_LEVEL),
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)
logger = logging.getLogger(__name__)

# Security
security = HTTPBearer()

# Initialize FastAPI app
app = FastAPI(
    title="Nasiko Router Service",
    description="AI-powered agent routing service",
    version="2.0.0",
)

# Add CORS middleware
app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.cors_origins_list,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Initialize orchestrator
orchestrator = RouterOrchestrator()


@app.get("/health")
async def health_check():
    """Health check endpoint."""
    try:
        health_status = await orchestrator.health_check()
        return health_status
    except Exception as e:
        logger.error(f"Health check failed: {e}")
        raise HTTPException(status_code=503, detail="Service unhealthy")


@app.get("/router/health")
async def health():
    return {"status": "ok"}


@app.post("/router")
async def process_request(
    session_id: str = Form(...),
    query: str = Form(...),
    route: Optional[str] = Form(None),
    files: Optional[List[UploadFile]] = File(
        None,
        max_length=settings.MAX_FILE_SIZE,
        description="Optional files to upload (PDF, TXT, DOCX, XLSX)",
    ),
    credentials: HTTPAuthorizationCredentials = Depends(security),
) -> StreamingResponse:
    """
    Process a user request through the router pipeline.

    Args:
        session_id: Unique session identifier
        query: User query text
        route: Optional direct route to specific agent
        files: Optional files to upload
        credentials: Bearer token credentials

    Returns:
        Streaming response with router processing updates

    Raises:
        HTTPException: For validation or processing errors
    """
    try:
        # Validate inputs
        validation_error = _validate_inputs(session_id, query)
        if validation_error:
            logger.error(f"Validation error: {validation_error}")
            raise HTTPException(status_code=400, detail=validation_error)

        # Process files
        files_to_forward = await _process_files(files)

        # Create request object
        request = UserRequest(session_id=session_id, query=query, route=route)
        logger.info(f"Processing request: {request}")
        logger.info(f"Files count: {len(files_to_forward)}")

        # Extract token
        token = credentials.credentials

        # Process through orchestrator
        return StreamingResponse(
            orchestrator.process_request(request, files_to_forward, token),
            media_type="application/json",
        )

    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"Unexpected error in process endpoint: {e}")
        raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")


@app.get("/metrics")
async def get_metrics():
    """Prometheus-compatible metrics for the request layer."""
    from fastapi.responses import PlainTextResponse
    text = get_stats().prometheus_text(get_cache(), get_limiter())
    return PlainTextResponse(text, media_type="text/plain; version=0.0.4")


# ── Admin endpoints ───────────────────────────────────────────────────────────

@app.get("/admin/stats/runtime")
async def admin_runtime_stats():
    """Full runtime stats: cache hits/misses/stores, queue depth, per-agent latency."""
    return get_stats().snapshot(get_cache(), get_limiter())


@app.post("/admin/cache/clear")
async def admin_cache_clear(agent_id: Optional[str] = None):
    """
    Clear cached responses.
    - agent_id provided → clear only that agent's entries.
    - no agent_id        → flush entire cache.
    """
    cache = get_cache()
    if agent_id:
        deleted = cache.clear_agent(agent_id)
        return {"cleared": deleted, "agent_id": agent_id}
    flushed = cache.flush()
    return {"flushed": flushed}


@app.put("/admin/cache/config")
async def admin_cache_config(ttl: Optional[int] = None, max_keys: Optional[int] = None):
    """
    Tune cache at runtime.
    - ttl      → new TTL in seconds.
    - max_keys → new max entry cap.
    """
    if ttl is None and max_keys is None:
        raise HTTPException(status_code=400, detail="Provide at least one of: ttl, max_keys")
    get_cache().configure(ttl=ttl, max_keys=max_keys)
    return get_cache().stats()


@app.put("/admin/limits/{agent_id:path}")
async def admin_set_limit(agent_id: str, rpm: int):
    """Set per-agent requests-per-minute limit. Takes effect immediately."""
    if rpm < 1:
        raise HTTPException(status_code=400, detail="rpm must be >= 1")
    get_limiter().set_limit(agent_id, rpm)
    return {"agent_id": agent_id, "new_limit_rpm": rpm}


def _validate_inputs(session_id: str, query: str) -> Optional[str]:
    """
    Validate request inputs.

    Args:
        session_id: Session identifier
        query: User query

    Returns:
        Error message if validation fails, None otherwise
    """
    if not session_id or not session_id.strip():
        return "session_id cannot be empty"
    logger.info(f"Session id: {session_id}")

    if not query or not query.strip():
        return "query cannot be empty"

    return None


async def _process_files(files: Optional[List[UploadFile]]) -> List[tuple]:
    """
    Process uploaded files for forwarding.

    Args:
        files: List of uploaded files

    Returns:
        List of file tuples ready for forwarding

    Raises:
        HTTPException: If file processing fails
    """
    files_to_forward = []

    if not files:
        return files_to_forward

    for file in files:
        try:
            if file.size and file.size > settings.MAX_FILE_SIZE:
                raise HTTPException(
                    status_code=413,
                    detail=f"File {file.filename} exceeds maximum size of {settings.MAX_FILE_SIZE} bytes",
                )

            content_bytes = await file.read()
            bio = BytesIO(content_bytes)
            bio.seek(0)

            files_to_forward.append(
                (
                    "files",
                    (
                        file.filename,
                        bio,
                        file.content_type or "application/octet-stream",
                    ),
                )
            )

        except HTTPException:
            raise
        except Exception as e:
            logger.error(f"Error reading file {file.filename}: {e}")
            raise HTTPException(
                status_code=400, detail=f"Failed to read file {file.filename}: {str(e)}"
            )

    return files_to_forward


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(
        "main:app",
        host=settings.HOST,
        port=settings.PORT,
        reload=settings.RELOAD,
        log_level=settings.LOG_LEVEL.lower(),
    )
