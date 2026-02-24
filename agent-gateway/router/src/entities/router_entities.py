from pydantic import BaseModel, Field


class RouterOutput(BaseModel):
    """The name of the agent and its capabilities."""

    agent_name: str = Field(
        description="The name of the appropriate agent to server user's request."
    )


class UserRequest(BaseModel):
    """Request body model for routing requests."""
    session_id: str
    query: str
    route: str | None = None


class RouterResponse(BaseModel):
    """Response model for router responses."""
    message: str
    is_int_response: bool
    agent_id: str | None
    url: str