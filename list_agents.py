"""FastAPI router for the MCP Manifest Generator.

Exposes endpoints to generate and retrieve MCP manifests.
Part of the Nasiko MCP Manifest Generator (R3).
"""

from __future__ import annotations

import logging
from typing import Any

from fastapi import APIRouter, HTTPException, status
from pydantic import BaseModel, Field

from .generator import MCPManifest, generate_manifest, load_manifest

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------

router = APIRouter(prefix="/manifest", tags=["manifest"])
logger = logging.getLogger("nasiko.mcp_manifest_generator")


# ---------------------------------------------------------------------------
# Models
# ---------------------------------------------------------------------------

class GenerateRequest(BaseModel):
    artifact_id: str = Field(..., description="Unique ID for the artifact")
    source_path: str = Field(..., description="Absolute path to the Python source file")


class InputSchemaResponse(BaseModel):
    type: str
    properties: dict[str, dict]
    required: list[str]


class MCPToolResponse(BaseModel):
    name: str
    description: str | None = None
    input_schema: InputSchemaResponse


class MCPManifestResponse(BaseModel):
    artifact_id: str
    generated_at: str
    tools: list[MCPToolResponse]


# ---------------------------------------------------------------------------
# Routes
# ---------------------------------------------------------------------------

@router.post(
    "/generate",
    response_model=MCPManifestResponse,
    status_code=status.HTTP_201_CREATED,
    summary="Generate an MCP manifest from source code",
)
async def api_generate_manifest(request: GenerateRequest) -> Any:
    """Parse a Python file and generate a manifest at /tmp/nasiko/{artifact_id}/manifest.json."""
    try:
        return generate_manifest(request.artifact_id, request.source_path)
    except ValueError as e:
        # Invalid artifact_id or Python syntax error
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
            detail=str(e)
        )
    except FileNotFoundError as e:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=str(e)
        )
    except Exception as e:
        logger.exception("Unexpected error during manifest generation")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Internal server error"
        )


@router.get(
    "/{artifact_id}",
    response_model=MCPManifestResponse,
    summary="Retrieve a previously generated MCP manifest",
)
async def api_get_manifest(artifact_id: str) -> Any:
    """Retrieve the manifest from /tmp/nasiko/{artifact_id}/manifest.json."""
    try:
        return load_manifest(artifact_id)
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
            detail=str(e)
        )
    except FileNotFoundError:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail=f"Manifest not found for artifact_id={artifact_id!r}"
        )
    except Exception as e:
        logger.exception("Unexpected error during manifest retrieval")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Internal server error"
        )
