"""
Refactored main router application with modular architecture.
"""

import logging
import os
from contextlib import asynccontextmanager
from io import BytesIO
from typing import List, Optional

import redis.asyncio as aioredis
from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.middleware.cors import CORSMiddleware
from fastapi.params import Depends
from fastapi.responses import RedirectResponse, StreamingResponse
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer
from fastapi.staticfiles import StaticFiles

from router.src.api import create_monitoring_router
from router.src.config import settings
from router.src.core import AgentHealthTracker, AgentRateLimiter, AgentResponseCache, EventEmitter
from router.src.entities import UserRequest
from router.src.services import RouterOrchestrator

# Configure logging
logging.basicConfig(
    level=getattr(logging, settings.LOG_LEVEL),
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)
logger = logging.getLogger(__name__)

# Security
security = HTTPBearer()


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Start Redis-backed services on startup and close cleanly on shutdown."""
    redis_client = aioredis.from_url(settings.REDIS_URL, decode_responses=True)

    emitter = EventEmitter(redis_client)

    cache = (
        AgentResponseCache(
            redis_client,
            settings.CACHE_DEFAULT_TTL,
            settings.MODEL_VERSION,
            settings.PROMPT_VERSION,
            emitter=emitter,
        )
        if settings.CACHE_ENABLED
        else None
    )

    rate_limiter = (
        AgentRateLimiter(
            redis_client,
            settings.RATE_LIMIT_DEFAULT_RPM,
            settings.RATE_LIMIT_MAX_QUEUE_SIZE,
            settings.RATE_LIMIT_QUEUE_TIMEOUT,
            emitter=emitter,
        )
        if settings.RATE_LIMIT_ENABLED
        else None
    )

    health = (
        AgentHealthTracker(redis_client, settings.RATE_LIMIT_MAX_QUEUE_SIZE)
        if settings.HEALTH_ENABLED
        else None
    )

    app.state.orchestrator = RouterOrchestrator(
        emitter=emitter, cache=cache, rate_limiter=rate_limiter, health=health
    )
    app.include_router(
        create_monitoring_router(cache, rate_limiter, health, emitter),
        prefix="/monitoring",
    )

    static_dir = os.path.join(os.path.dirname(__file__), "static")
    if os.path.isdir(static_dir):
        app.mount("/dashboard", StaticFiles(directory=static_dir, html=True), name="dashboard")
        logger.info(f"Dashboard mounted at /dashboard from {static_dir}")

    logger.info(
        f"Router started — cache={'on' if cache else 'off'}, "
        f"rate_limit={'on' if rate_limiter else 'off'}, "
        f"health={'on' if health else 'off'}"
    )

    yield

    await redis_client.aclose()
    logger.info("Redis connection closed")


# Initialize FastAPI app
app = FastAPI(
    title="Nasiko Router Service",
    description="AI-powered agent routing service with caching, rate limiting, and health tracking",
    version="3.0.0",
    lifespan=lifespan,
)

# Add CORS middleware
app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.cors_origins_list,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.get("/health")
async def health_check():
    """Health check endpoint."""
    try:
        health_status = await app.state.orchestrator.health_check()
        return health_status
    except Exception as e:
        logger.error(f"Health check failed: {e}")
        raise HTTPException(status_code=503, detail="Service unhealthy")


@app.get("/router/health")
async def router_health():
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

    Returns:
        Streaming response with router processing updates
    """
    try:
        validation_error = _validate_inputs(session_id, query)
        if validation_error:
            logger.error(f"Validation error: {validation_error}")
            raise HTTPException(status_code=400, detail=validation_error)

        files_to_forward = await _process_files(files)

        request = UserRequest(session_id=session_id, query=query, route=route)
        logger.info(f"Processing request: {request}")
        logger.info(f"Files count: {len(files_to_forward)}")

        token = credentials.credentials

        return StreamingResponse(
            app.state.orchestrator.process_request(request, files_to_forward, token),
            media_type="application/json",
        )

    except HTTPException:
        raise
    except Exception as e:
        logger.error(f"Unexpected error in process endpoint: {e}")
        raise HTTPException(status_code=500, detail=f"Internal server error: {str(e)}")


@app.get("/metrics")
async def get_metrics():
    """Redirect to the monitoring overview endpoint."""
    return RedirectResponse(url="/monitoring/overview")


def _validate_inputs(session_id: str, query: str) -> Optional[str]:
    if not session_id or not session_id.strip():
        return "session_id cannot be empty"
    logger.info(f"Session id: {session_id}")
    if not query or not query.strip():
        return "query cannot be empty"
    return None


async def _process_files(files: Optional[List[UploadFile]]) -> List[tuple]:
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
