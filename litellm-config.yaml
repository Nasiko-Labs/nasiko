"""Nasiko ingestion — artifact-type detection and upload validation.

Provides AST-based detection of uploaded artifact types (MCP server,
LangChain agent, CrewAI agent) with loud failure on ambiguity.
"""

from nasiko.app.ingestion.detector import detect_artifact_type
from nasiko.app.ingestion.models import IngestionRecord, ArtifactType, DetectionConfidence
from nasiko.app.ingestion.exceptions import AmbiguousArtifactError

__all__ = [
    "detect_artifact_type",
    "IngestionRecord",
    "ArtifactType",
    "DetectionConfidence",
    "AmbiguousArtifactError",
]
