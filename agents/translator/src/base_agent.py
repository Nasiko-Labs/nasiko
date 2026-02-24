import logging
from abc import ABC, abstractmethod

from document_parser import DocumentParser
from engine import Engine

logger = logging.getLogger(__name__)


class BaseAgent(ABC):
    """Abstract base class for document agents."""

    def __init__(
        self,
        model: str = "gpt-4o",
        mongo_url: str = "mongodb://localhost:27017",
        db_name: str = "chat_history",
        collection_name: str = "messages",
    ):
        logger.info(
            f"Initializing BaseAgent with DB={db_name}, collection={collection_name}"
        )
        self.agent = Engine(
            model=model,
            mongo_url=mongo_url,
            db_name=db_name,
            collection_name=collection_name,
        )
        self.document_parser = DocumentParser()

    @abstractmethod
    def get_response(self, *args, **kwargs) -> str:
        pass
