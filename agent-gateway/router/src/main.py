"""
Refactored main router application with modular architecture.
"""

import logging
from io import BytesIO
from typing import List, Optional, Dict, Any

from fastapi import FastAPI, File, Form, UploadFile, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.params import Depends
from fastapi.responses import StreamingResponse
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

from router.src.config import settings
from router.src.entities import UserRequest
from router.src.services import RouterOrchestrator
from router.src.services.rate_limiter_service import agent_rate_limiter

# Configure logging
logging.basicConfig(
    level=getattr(logging, settings.LOG_LEVEL),
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)
logger = logging.getLogger(__name__)

# Security
security = HTTPBearer(auto_error=False)

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
    credentials: Optional[HTTPAuthorizationCredentials] = Depends(security),
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
        token = credentials.credentials if credentials else ""

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


@app.get("/router/limits")
async def get_all_limits():
    """Get rate limit statistics for all agents."""
    return agent_rate_limiter.get_all_stats()


@app.get("/router/limits/{agent_id:path}")
async def get_agent_limit(agent_id: str):
    """Get rate limit statistics for a specific agent."""
    stats = agent_rate_limiter.get_agent_stats(agent_id)
    if not stats:
        raise HTTPException(status_code=404, detail="Agent not found in rate limiter")
    return stats


@app.post("/router/limits")
async def update_agent_limit(config: Dict[str, Any]):
    """
    Update rate limit for an agent.
    Expected payload: {"agent_id": "...", "limit": 10}
    """
    agent_id = config.get("agent_id")
    limit = config.get("limit")
    
    if not agent_id or limit is None:
        raise HTTPException(status_code=400, detail="Missing agent_id or limit")
        
    try:
        limit_int = int(limit)
        await agent_rate_limiter.set_limit(agent_id, limit_int)
        return {"status": "success", "agent_id": agent_id, "limit": limit_int}
    except ValueError:
        raise HTTPException(status_code=400, detail="Limit must be an integer")


@app.get("/metrics")
async def get_metrics():
    """Get router service metrics including rate limiting stats."""
    agent_stats = agent_rate_limiter.get_all_stats()
    
    total_active = sum(s.active_requests for s in agent_stats)
    total_queued = sum(s.queued_requests for s in agent_stats)
    total_requests = sum(s.total_requests for s in agent_stats)
    
    avg_wait = sum(s.avg_wait_time_ms for s in agent_stats) / len(agent_stats) if agent_stats else 0.0
    avg_resp = sum(s.avg_response_time_ms for s in agent_stats) / len(agent_stats) if agent_stats else 0.0
    total_errors = sum(s.error_count for s in agent_stats)
    
    return {
        "requests_processed": total_requests,
        "active_requests": total_active,
        "queued_requests": total_queued,
        "agent_specific_stats": agent_stats,
        "average_wait_time_ms": avg_wait,
        "average_response_time_ms": avg_resp,
        "total_errors": total_errors,
        "error_rate": total_errors / total_requests if total_requests > 0 else 0.0,
    }


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
