"""
Chat Repository - Chat sessions and history operations
"""
import random
from datetime import datetime, timezone
from typing import Optional
from .base_repository import BaseRepository


class ChatRepository(BaseRepository):
    """Repository for chat sessions and history operations"""
    
    def __init__(self, db, logger):
        super().__init__(db, logger)
        self.ChatSessionCollection = db["chat-sessions"]
        self.ChatHistoryCollection = db["chat-history"]
    
    async def ensure_indexes(self):
        """Ensure chat collections indexes"""
        try:
            # Chat Session collection indexes
            await self.ChatSessionCollection.create_index("session_id", unique=True)
            await self.ChatSessionCollection.create_index("user_id")
            
            # Chat History collection indexes
            await self.ChatHistoryCollection.create_index("session_id")
            await self.ChatHistoryCollection.create_index("timestamp")
            await self.ChatHistoryCollection.create_index(
                [("session_id", 1), ("timestamp", 1)]
            )
            
            self.logger.info("Chat collections indexes initialized successfully")
        except Exception as e:
            self.logger.warning(f"Error ensuring chat indexes: {e}")
    
    def _generate_session_title(self) -> str:
        """Generate a random creative session title"""
        adjectives = [
            "Quick", "Bright", "Creative", "Smart", "Fresh", "New", "Morning", 
            "Evening", "Midnight", "Focused", "Deep", "Brief", "Productive", 
            "Brainstorm", "Strategic", "Casual", "Important",
        ]
        nouns = [
            "Chat", "Discussion", "Session", "Talk", "Conversation", "Meeting", 
            "Dialogue", "Exchange", "Brainstorm", "Planning", "Review", "Sync", 
            "Catchup", "Huddle", "Workshop",
        ]
        return f"{random.choice(adjectives)} {random.choice(nouns)}"
    
    async def create_session(
        self, user_id: str, session_id: str, agent_id: Optional[str] = None, agent_url: Optional[str] = None
    ):
        """Insert session id and user id"""
        created_at = datetime.now(timezone.utc)
        title = self._generate_session_title()
        
        try:
            session_document = {
                "user_id": user_id,
                "session_id": session_id,
                "created_at": created_at,
                "updated_at": created_at,
                "title": title,
            }
            
            # Only add agent_id if it's provided
            if agent_id:
                session_document["agent_id"] = agent_id
            
            # Only add agent_url if it's provided
            if agent_url:
                session_document["agent_url"] = agent_url
                
            await self.ChatSessionCollection.insert_one(session_document)
            
            return {"created_at": created_at, "title": title, "agent_id": agent_id, "agent_url": agent_url}
        except Exception as e:
            self.logger.error(
                f"Failed to insert session {session_id} for user {user_id}: {e}"
            )
            raise
    
    async def delete_session(self, session_id: str, user_id: str):
        """Delete session id"""
        try:
            result = await self.ChatSessionCollection.delete_one(
                {"session_id": session_id, "user_id": user_id}
            )
            return result.deleted_count > 0
        except Exception as e:
            self.logger.error(
                f"Failed to delete session {session_id} for user {user_id}: {e}"
            )
            raise
    
    async def get_session_history(
        self,
        user_id: str,
        limit: int = 20,
        cursor: Optional[str] = None,
        direction: str = "after",
    ):
        """Retrieve paginated session history for a user"""
        try:
            query = {"user_id": user_id}
            
            total_count = await self.ChatSessionCollection.count_documents(query)
            
            if cursor:
                try:
                    cursor_time = datetime.fromisoformat(cursor)
                    if cursor_time.tzinfo is None:
                        cursor_time = cursor_time.replace(tzinfo=timezone.utc)
                    
                    if direction == "after":
                        query["created_at"] = {"$gt": cursor_time}
                    else:  # "before"
                        query["created_at"] = {"$lt": cursor_time}
                
                except (ValueError, AttributeError) as e:
                    self.logger.warning(f"Invalid cursor format: {cursor}, error: {e}")
                    return None
            
            sessions = (
                await self.ChatSessionCollection.find(query)
                .sort("created_at", -1)
                .limit(limit + 1)
                .to_list(length=limit + 1)
            )
            
            has_more = len(sessions) > limit
            if has_more:
                sessions = sessions[:limit]
            
            next_cursor = None
            prev_cursor = None
            
            if sessions:
                if has_more and direction == "after":
                    next_cursor = sessions[-1]["created_at"].isoformat()
                if has_more and direction == "before":
                    prev_cursor = sessions[0]["created_at"].isoformat()
                
                if cursor:
                    if direction == "after":
                        prev_cursor = sessions[0]["created_at"].isoformat()
                    else:
                        next_cursor = sessions[-1]["created_at"].isoformat()
            
            return {
                "messages": sessions,
                "total_count": total_count,
                "has_more": has_more,
                "next_cursor": next_cursor,
                "prev_cursor": prev_cursor,
            }
        except Exception as e:
            self.logger.error(
                f"Database error fetching session history for user {user_id}: {e}"
            )
            raise
    
    async def get_chat_history(
        self,
        user_id: str,
        session_id: str,
        limit: int = 20,
        cursor: Optional[str] = None,
        direction: str = "after",
    ):
        """Get paginated chat history for a session"""
        try:
            # Verify session exists and belongs to user
            session_doc = await self.ChatSessionCollection.find_one(
                {"session_id": session_id, "user_id": user_id}
            )
            
            if not session_doc:
                self.logger.warning(
                    f"Session {session_id} not found or doesn't belong to user {user_id}"
                )
                return None
            
            query = {"session_id": session_id}
            
            if cursor:
                try:
                    cursor_time = datetime.fromisoformat(cursor)
                    # Ensure timezone-aware for comparison
                    if cursor_time.tzinfo is None:
                        cursor_time = cursor_time.replace(tzinfo=timezone.utc)
                    
                    if direction == "after":
                        # Get NEWER messages (later in time)
                        query["timestamp"] = {"$gt": cursor_time}
                    else:  # "before"
                        # Get OLDER messages (earlier in time)
                        query["timestamp"] = {"$lt": cursor_time}
                
                except (ValueError, AttributeError) as e:
                    self.logger.warning(f"Invalid cursor format: {cursor}, error: {e}")
                    return None
            
            total_count = await self.ChatHistoryCollection.count_documents(
                {"session_id": session_id}
            )
            
            messages = (
                await self.ChatHistoryCollection.find(query)
                .sort("timestamp", 1)  # Always ascending (oldest → newest)
                .limit(limit + 1)
                .to_list(length=limit + 1)
            )
            
            has_more = len(messages) > limit
            if has_more:
                messages = messages[:limit]
            
            next_cursor = None
            prev_cursor = None
            
            if messages:
                if has_more and direction == "after":
                    next_cursor = messages[-1]["timestamp"].isoformat()
                if has_more and direction == "before":
                    prev_cursor = messages[0]["timestamp"].isoformat()
                
                if cursor:
                    if direction == "after":
                        prev_cursor = messages[0]["timestamp"].isoformat()
                    else:
                        next_cursor = messages[-1]["timestamp"].isoformat()
            
            return {
                "messages": messages,
                "total_count": total_count,
                "has_more": has_more,
                "next_cursor": next_cursor,
                "prev_cursor": prev_cursor,
            }
        
        except Exception as e:
            self.logger.error(
                f"Failed to retrieve chat history for session: {session_id} for user {user_id}: {e}"
            )
            raise