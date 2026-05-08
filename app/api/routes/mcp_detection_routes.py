"""
MCP Detection Routes
"""

from fastapi import APIRouter, Depends, UploadFile, File
from app.api.handlers.mcp_detection_handler import MCPDetectionHandler
from app.service.service import Service
import logging

router = APIRouter(prefix="/api/v1/mcp", tags=["MCP Detection"])
logger = logging.getLogger(__name__)


def get_handler():
    """Dependency to get MCP detection handler"""
    service = Service(logger)
    return MCPDetectionHandler(service, logger)


@router.post("/detect")
async def detect_artifact_type(
    file: UploadFile = File(...),
    handler: MCPDetectionHandler = Depends(get_handler)
):
    """
    Detect if uploaded artifact is an agent or MCP server
    
    Returns artifact type, confidence score, and detected patterns
    """
    return await handler.detect_artifact_type(file)


@router.get("/health")
async def mcp_health():
    """Health check for MCP detection service"""
    return {
        "status": "healthy",
        "service": "mcp_detection",
        "version": "1.0.0"
    }
