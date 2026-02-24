import logging
from datetime import datetime, timezone

from langchain_core.chat_history import BaseChatMessageHistory
from langchain_core.messages import messages_from_dict, messages_to_dict
from pymongo import MongoClient

logger = logging.getLogger(__name__)


class MongoChatHistory(BaseChatMessageHistory):
    def __init__(
        self,
        session_id: str,
        mongo_url: str,
        db_name: str,
        collection_name: str = "messages",
    ):
        logger.debug(
            f"Connecting to MongoDB at {mongo_url}, db={db_name}, collection={collection_name}"
        )
        self.session_id = session_id
        self.client = MongoClient(mongo_url)
        self.collection = self.client[db_name][collection_name]

    def add_message(self, message):
        doc = {
            "session_id": self.session_id,
            "history": messages_to_dict([message])[0],
            "created_at": datetime.now(timezone.utc),
        }
        self.collection.insert_one(doc)
        logger.info(f"Added message for session {self.session_id}")

    def clear(self):
        result = self.collection.delete_many({"session_id": self.session_id})
        logger.info(
            f"Cleared {result.deleted_count} messages for session {self.session_id}"
        )

    @property
    def messages(self):
        docs = self.collection.find({"session_id": self.session_id}).sort(
            "created_at", 1
        )
        messages = []
        for doc in docs:
            msg = messages_from_dict([doc["history"]])[0]
            msg.created_at = doc.get("created_at")
            messages.append(msg)

        logger.debug(f"Fetched {len(messages)} messages for session {self.session_id}")
        return messages
