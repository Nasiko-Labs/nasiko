"""
Pydantic models for A2A protocol.
"""
import uuid
from typing import List, Optional, Dict, Any, Literal
from pydantic import BaseModel, Field

class MessagePart(BaseModel):
    kind: str
    text: Optional[str] = None

class Message(BaseModel):
    role: str
    parts: List[MessagePart]
    messageId: Optional[str] = None

class JsonRpcParams(BaseModel):
    session_id: Optional[str] = None
    message: Message

class JsonRpcRequest(BaseModel):
    jsonrpc: Literal["2.0"]
    id: str
    method: str
    params: JsonRpcParams

class ArtifactPart(BaseModel):
    kind: str = "text"
    text: str

class Artifact(BaseModel):
    id: str = Field(default_factory=lambda: str(uuid.uuid4()))
    kind: str = "text"
    parts: List[ArtifactPart]

class TaskStatus(BaseModel):
    state: str
    timestamp: str

class Task(BaseModel):
    id: str
    kind: str = "task"
    status: TaskStatus
    artifacts: List[Artifact] = []
    contextId: Optional[str] = None

class JsonRpcResponse(BaseModel):
    jsonrpc: Literal["2.0"] = "2.0"
    id: str
    result: Task
