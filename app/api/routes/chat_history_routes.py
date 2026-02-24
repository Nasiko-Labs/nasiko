from typing import Optional

from fastapi import Depends, Query, Body
from fastapi.routing import APIRouter

from app.api.auth import get_user_id_from_token
from app.api.handlers import HandlerFactory
from app.entity.entity import (
    CreateSessionRequest,
    SessionResponse,
    ChatHistoryResponse,
    SessionHistoryResponse,
    MessageResponse,
)

def create_chat_history_routes(handlers: HandlerFactory) -> APIRouter:
    """Create chat history routes"""
    router = APIRouter(prefix="/chat/session", tags=["chat_history"])

    @router.post(
        path="",
        response_model=SessionResponse,
        summary="Create new session",
        description="Create a new chat session for the authenticated user with optional agent ID and URL",
    )
    async def create_session(
        request: CreateSessionRequest = Body(
            default_factory=CreateSessionRequest,
            description="Session creation request with optional agent_id and agent_url"
        ),
        user_id: str = Depends(get_user_id_from_token),
    ):
        """Create and return session id"""
        return await handlers.chat_history.create_session(user_id, request.agent_id, request.agent_url)

    @router.delete(
        path="/{session_id}",
        response_model=MessageResponse,
        summary="Delete Session",
        description="Delete session of the authenticated user",
    )
    async def delete_session(
        session_id: str, user_id: str = Depends(get_user_id_from_token)
    ):
        """Delete session"""
        return await handlers.chat_history.delete_session(
            user_id=user_id, session_id=session_id
        )

    @router.get(
        path="/list",
        response_model=SessionHistoryResponse,
        summary="Get paginated session history",
        description="Get session history for the authenticated user",
    )
    async def get_session_history(
        user_id: str = Depends(get_user_id_from_token),
        limit: int = Query(
            10, ge=1, le=20, alias="limit", description="Number of messages per page"
        ),
        cursor: Optional[str] = Query(
            None, alias="cursor", description="Cursor for pagination"
        ),
        direction: str = Query(
            "after",
            regex="^(before|after)$",
            alias="direction",
            description="Direction for pagination",
        ),
    ):
        """Get session history"""
        return await handlers.chat_history.get_session_history(
            user_id=user_id,
            limit=limit,
            cursor=cursor,
            direction=direction,
        )

    @router.get(
        path="/{session_id}",
        response_model=ChatHistoryResponse,
        summary="Get chat history",
        description="Get the chat history for the authenticated user by session id",
    )
    async def get_chat_history(
        session_id: str,
        user_id: str = Depends(get_user_id_from_token),
        limit: int = Query(
            50, ge=1, le=100, alias="limit", description="Number of messages per page"
        ),
        cursor: Optional[str] = Query(
            None, alias="cursor", description="Cursor for pagination"
        ),
        direction: str = Query(
            "after",
            pattern="^(before|after)$",
            alias="direction",
            description="Direction for pagination",
        ),
    ):
        """Get paginated chat history by session id"""
        return await handlers.chat_history.get_chat_history(
            user_id=user_id,
            session_id=session_id,
            limit=limit,
            cursor=cursor,
            direction=direction,
        )

    return router
