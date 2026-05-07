from enum import Enum
from typing import Optional
from datetime import datetime, timezone
from pydantic import BaseModel, Field
import uuid


class ArtifactType(str, Enum):
    MCP_SERVER = "MCP_SERVER"
    LANGCHAIN_AGENT = "LANGCHAIN_AGENT"
    CREWAI_AGENT = "CREWAI_AGENT"


class DetectionConfidence(str, Enum):
    HIGH = "HIGH"
    AMBIGUOUS = "AMBIGUOUS"


class IngestionRecord(BaseModel):
    artifact_id: str = Field(..., description="alphanumeric + _- only")
    source_path: str = Field(..., description="/tmp/nasiko/uploads/{uuid}/")
    artifact_type: ArtifactType
    confidence: DetectionConfidence
    created_at: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))
    entry_point: str
    detected_framework: str
    requirements_path: Optional[str]
    agentcard_exists: bool
