import logging

from langchain_core.prompts import ChatPromptTemplate
from langchain_core.runnables.history import RunnableWithMessageHistory

from chat_history import MongoChatHistory
from llm import LLMClient

logger = logging.getLogger(__name__)


class Engine:
    def __init__(
        self,
        model: str = "gpt-4o",
        mongo_url: str = "mongodb://localhost:27017",
        db_name: str = "chat_history",
        collection_name: str = "messages",
    ):
        logger.info(f"Initializing Engine with model={model}")
        self.llm_client = LLMClient(model)

        def get_history(session_id: str):
            logger.debug(f"Creating MongoChatHistory for session {session_id}")
            return MongoChatHistory(
                session_id=session_id,
                mongo_url=mongo_url,
                db_name=db_name,
                collection_name=collection_name,
            )

        prompt = ChatPromptTemplate.from_messages(
            [
                ("system", "{system_prompt}"),
                ("placeholder", "{history}"),
                ("human", "{user_prompt}"),
            ]
        )

        chain = prompt | self.llm_client.llm
        self.chain_with_history = RunnableWithMessageHistory(
            runnable=chain,
            get_session_history=get_history,
            input_messages_key="user_prompt",
            history_messages_key="history",
        )

    def chat(
        self, system_prompt: str, user_prompt: str, session_id: str = "default"
    ) -> str:
        logger.info(f"Running chat for session {session_id}")
        response = self.chain_with_history.invoke(
            {"system_prompt": system_prompt, "user_prompt": user_prompt},
            config={"configurable": {"session_id": session_id}},
        )
        logger.debug(
            f"Chat response for session {session_id}: {response.content[:100]}"
        )
        return response.content
