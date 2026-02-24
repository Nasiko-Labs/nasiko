from typing import Optional

from fastapi import HTTPException, status

from app.api.handlers import BaseHandler
from app.entity.entity import (
    SessionResponse,
    SessionData,
    ChatHistory,
    ChatHistoryResponse,
    SessionHistory,
    SessionHistoryResponse,
    PaginationMetaData,
    MessageResponse,
)
from app.service.chat_history_service import ChatHistoryService


class ChatHistoryHandler(BaseHandler):
    """Handler for chat history operations"""

    def __init__(self, service, logger):
        super().__init__(service, logger)
        self.chat_history_service = ChatHistoryService(service.repo, logger)

    async def create_session(
        self, user_id: str, agent_id: Optional[str] = None, agent_url: Optional[str] = None
    ) -> SessionResponse | None:
        """Create a new session for user"""
        try:
            session_data = await self.chat_history_service.create_session(
                user_id, agent_id, agent_url
            )

            if not session_data:
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail="Failed to create session",
                )

            return SessionResponse(
                data=SessionData(
                    session_id=session_data["session_id"],
                    created_at=session_data["created_at"],
                    title=session_data["title"],
                    agent_id=session_data.get("agent_id"),
                    agent_url=session_data.get("agent_url"),
                ),
                status_code=201,
                message="Session created successfully",
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("Create session", e)

    async def delete_session(self, user_id: str, session_id: str):
        """Delete session for user"""
        try:
            result = await self.chat_history_service.delete_session(
                user_id=user_id, session_id=session_id
            )
            if not result:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"Session {session_id} not found",
                )
            return MessageResponse(
                status_code=204,
                message="Session deleted successfully",
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("Delete session", e)

    async def get_session_history(
        self,
        user_id: str,
        limit: int = 20,
        cursor: Optional[str] = None,
        direction: str = "after",
    ):
        try:
            result = await self.chat_history_service.get_session_history(
                user_id=user_id,
                limit=limit,
                cursor=cursor,
                direction=direction,
            )

            collection = []
            total_count = 0
            has_more = False
            next_cursor = None
            prev_cursor = None

            if result and result.get("messages"):
                collection = [
                    SessionHistory(
                        session_id=history["session_id"],
                        title=history["title"],
                        agent_id=history.get("agent_id"),
                        agent_url=history.get("agent_url"),
                    )
                    for history in result["messages"]
                ]
                total_count = result["total_count"]
                has_more = result["has_more"]
                next_cursor = result.get("next_cursor", None)
                prev_cursor = result.get("prev_cursor", None)

            return SessionHistoryResponse(
                data=collection,
                pagination=PaginationMetaData(
                    total_count=total_count,
                    returned_count=len(collection),
                    has_more=has_more,
                    next_cursor=next_cursor,
                    prev_cursor=prev_cursor,
                ),
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("Session history", e)

    async def get_chat_history(
        self,
        user_id,
        session_id,
        limit: int = 50,
        cursor: Optional[str] = None,
        direction: str = "after",
    ):
        """Return chat history"""
        try:
            result = await self.chat_history_service.get_chat_history(
                user_id=user_id,
                session_id=session_id,
                limit=limit,
                cursor=cursor,
                direction=direction,
            )

            collection = []
            total_count = 0
            has_more = False
            next_cursor = None
            prev_cursor = None

            if result and result.get("messages"):
                collection = [
                    ChatHistory(
                        role=history["role"],
                        content=history["content"],
                        timestamp=history["timestamp"],
                    )
                    for history in result["messages"]
                ]
                total_count = result["total_count"]
                has_more = result["has_more"]
                next_cursor = result.get("next_cursor", None)
                prev_cursor = result.get("prev_cursor", None)

            return ChatHistoryResponse(
                data=collection,
                pagination=PaginationMetaData(
                    total_count=total_count,
                    returned_count=len(collection),
                    has_more=has_more,
                    next_cursor=next_cursor,
                    prev_cursor=prev_cursor,
                ),
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("Chat history", e)
