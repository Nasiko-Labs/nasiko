"""
Platform log API models.
"""

from datetime import datetime
from typing import Any, Dict, List, Literal, Optional

from pydantic import BaseModel, Field


class PlatformLogEntry(BaseModel):
    id: Optional[str] = None
    timestamp: str
    level: str
    message: str
    service: str = "nasiko-backend"
    logger: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = None


class PlatformLogsResponse(BaseModel):
    logs: List[PlatformLogEntry]
    total: int
    limit: int
    skip: int
    level_filter: Optional[str] = None


class PlatformLogCreate(BaseModel):
    message: str = Field(..., min_length=1, max_length=4096)
    level: Literal["INFO", "WARNING", "ERROR"] = "INFO"
    service: str = Field(default="nasiko-backend", max_length=128)
    metadata: Optional[Dict[str, Any]] = None
