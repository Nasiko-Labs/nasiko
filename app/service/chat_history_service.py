import uuid

class ChatHistoryService:
    def __init__(
        self,
        repository,
        logger,
    ):
        self.repository = repository
        self.logger = logger

    async def create_session(self, user_id: str, agent_id: str | None = None, agent_url: str | None = None):
        log_parts = [f"Creating new session for user {user_id}"]
        if agent_id:
            log_parts.append(f"with agent_id: {agent_id}")
        if agent_url:
            log_parts.append(f"with agent_url: {agent_url}")
        self.logger.info(" ".join(log_parts))
        
        session_id = uuid.uuid4().hex
        self.logger.debug(f"Created session: {session_id}")
        try:
            session_data = await self.repository.create_session(
                user_id, session_id, agent_id, agent_url
            )
            self.logger.info(
                f"Generated session: {session_id} with title: '{session_data['title']}' for user: {user_id}"
            )
            return {
                "session_id": session_id,
                "created_at": session_data["created_at"].isoformat(),
                "title": session_data["title"],
                "agent_id": session_data.get("agent_id"),
                "agent_url": session_data.get("agent_url"),
            }
        except Exception as e:
            self.logger.error(f"Failed to generate session id for user: {user_id}: {e}")
            raise

    async def delete_session(self, session_id: str, user_id: str):
        try:
            self.logger.info(f"Deleting session_id: {session_id} for user: {user_id}")
            result = await self.repository.delete_session(
                session_id=session_id, user_id=user_id
            )
            if not result:
                self.logger.warning(
                    f"Session: {session_id} not found or doesn't belong to user: {user_id}"
                )
                return None
            self.logger.info(f"Deleted session_id: {session_id} for user: {user_id}")
            return True
        except Exception as e:
            self.logger.error(f"Failed to delete session: {session_id}: {e}")
            raise

    async def get_session_history(
        self,
        user_id: str,
        limit: int = 20,
        cursor: str | None = None,
        direction: str = "after",
    ):
        """Return chat history for session"""
        try:
            self.logger.info(f"Retrieving chat history for session: {user_id}")
            result = await self.repository.get_session_history(
                user_id=user_id,
                limit=limit,
                cursor=cursor,
                direction=direction,
            )

            if not result or not result.get("messages"):
                self.logger.warning(f"Session history not found for user: {user_id}")
                return None

            self.logger.info(f"Retrieved session history for user: {user_id}")
            self.logger.debug(f"Retrieved session history: {result['messages']}")
            return result
        except Exception as e:
            self.logger.error(
                f"Failed to retrieve session history for user: {user_id}: {e}"
            )
            raise

    async def get_chat_history(
        self,
        user_id: str,
        session_id: str,
        limit: int = 50,
        cursor: str | None = None,
        direction: str = "after",
    ):
        """Return chat history of a user by session_id"""
        try:
            self.logger.info(
                f"Retrieving chat history for session: {session_id} for user: {user_id} "
            )
            result = await self.repository.get_chat_history(
                user_id=user_id,
                session_id=session_id,
                limit=limit,
                cursor=cursor,
                direction=direction,
            )

            if not result or not result.get("messages"):
                self.logger.warning(
                    f"Chat history not found for session: {session_id} for user: {user_id}"
                )
                return None
            self.logger.info(
                f"Retrieved {len(result['messages'])} messages for session {session_id}"
            )
            self.logger.debug(f"Retrieved chat history: {result['messages']}")
            return result
        except Exception as e:
            self.logger.error(
                f"Failed to retrieve chat_history for session: {session_id}: {e}"
            )
            raise
