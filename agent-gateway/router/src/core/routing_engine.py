"""
Routing engine service for AI-powered agent selection.
"""

import json
import logging
from typing import Any, Dict, List, Tuple

import numpy as np

from langchain_core.messages import SystemMessage
from langchain_core.prompts import ChatPromptTemplate
from langchain_core.documents import Document
from langchain_openai import ChatOpenAI, OpenAIEmbeddings
from langchain_community.vectorstores import FAISS

from router.src.config import settings
from router.src.entities import RouterOutput

logger = logging.getLogger(__name__)


class RoutingEngineError(Exception):
    """Custom exception for routing engine errors."""


class RoutingEngine:
    """Service for AI-powered agent routing and selection."""

    def __init__(self):
        self.llm = self._create_llm()
        self.embedding_model = self._create_embedding_model()

    def _create_llm(self) -> ChatOpenAI:
        """Create LLM instance for routing decisions."""
        # return ChatOpenAI(
        #     model="google/gemini-2.5-flash",
        #     temperature=0,
        #     api_key=settings.OPENROUTER_API_KEY,
        #     base_url="https://openrouter.ai/api/v1",
        # ).with_structured_output(RouterOutput)

        return ChatOpenAI(
            model="gpt-4o-mini",
            temperature=0,
            api_key=settings.OPENAI_API_KEY,
        ).with_structured_output(RouterOutput)

    def _create_embedding_model(self) -> OpenAIEmbeddings:
        """Create OpenAI embeddings instance."""

        return OpenAIEmbeddings(
            model=settings.RERANKING_EMBEDDING_MODEL,
            openai_api_key=settings.OPENAI_API_KEY,
        )

    def route_query(
        self,
        message: str,
        conversation_history: List[Dict[str, str]],
        agent_cards: List[Dict[str, Any]],
        vectorstore: FAISS,
    ) -> Tuple[List[str], List[float], List[str], RouterOutput]:
        """
        Route a user query to the most appropriate agent.

        Args:
            message: User's query message
            conversation_history: User's conversation history in the current session
            agent_cards: List of available agent card dictionaries
            vectorstore: FAISS vector store for similarity search

        Returns:
            Tuple of (shortlisted_agents, router_output)

        Raises:
            RoutingEngineError: If routing fails
        """
        try:
            if len(agent_cards) < 15:
                first_shortlist = [agent["name"] for agent in agent_cards]
                second_shortlist = [agent["name"] for agent in agent_cards]
                similarity_score = [1.0] * len(agent_cards)
                shortlisted_agent_cards = agent_cards
            else:
                (
                    first_shortlist,
                    similarity_score,
                    second_shortlist,
                    shortlisted_agent_cards,
                ) = self._semantic_search_with_reranking(
                    message, conversation_history, agent_cards, vectorstore
                )

            # Then use LLM to make final selection
            router_output = self._llm_route(
                message, conversation_history, shortlisted_agent_cards
            )
            return first_shortlist, similarity_score, second_shortlist, router_output

        except Exception as e:
            error_msg = f"Routing failed: {str(e)}"
            logger.error(error_msg)
            raise RoutingEngineError(error_msg) from e

    def _prepare_conversation_history(self, conversation_history: List[Dict[str, str]]):
        conversation_history_str = ""
        for turn in conversation_history:
            conversation_history_str += f"{turn['role']}: {turn['content']}"
        return conversation_history_str

    def _cosine_similarity(self, vector_1, vector_2):
        return np.dot(vector_1, vector_2) / (
            np.linalg.norm(vector_1) * np.linalg.norm(vector_2)
        )

    def _rerank_agents(
        self,
        first_search_results: List[Document],
        first_search_embeddings: List[List[float]],
        message: str,
        conversation_history: List[Dict[str, str]],
        k: int = 2,
    ) -> List[str]:
        """
        Re-rank agents based on conversation history.

        Args:
            first_search_results: List of search results from semantic search
            first_search_embeddings: Embeddings corresponding to search results
            conversation_history: User's conversation history
            k: Number of agents to return

        Returns:
            List of agent names
        """
        conversation_history_str = self._prepare_conversation_history(
            conversation_history
        )
        query = conversation_history_str + f"Human: {message}"
        query_embedding = self.embedding_model.embed_query(query)
        scores = []
        for i, embedding in enumerate(first_search_embeddings):
            similarity = self._cosine_similarity(embedding, query_embedding)
            scores.append((i, similarity))

        scores.sort(key=lambda x: x[1], reverse=True)

        second_shortlist = []
        for i, _ in scores[:k]:
            second_shortlist.append(first_search_results[i].metadata["name"])

        return second_shortlist

    def _semantic_search_with_reranking(
        self,
        message: str,
        conversation_history: List[Dict[str, str]],
        agent_cards: List[Dict[str, Any]],
        vectorstore: FAISS,
    ) -> Tuple[List[str], List[float], List[str], List[Dict[str, Any]]]:
        """
        Perform semantic search to shortlist relevant agents.

        Args:
            message: User's query message
            conversation_history: User's conversation history
            agent_cards: List of available agent cards
            vectorstore: FAISS vector store

        Returns:
            Tuple of (first_shortlist, second_shortlist, shortlisted_agent_cards)
        """
        try:
            k = 15

            # Embed the query
            query_embedding = np.array(
                [self.embedding_model.embed_query(message)], dtype=np.float32
            )

            # Directly search the FAISS index to get indices and distances
            distances, indices = vectorstore.index.search(query_embedding, k)
            distances = distances[0]  # Get the first (and only) query's results
            indices = indices[0]

            # Retrieve documents and embeddings using the indices
            search_results = []
            search_embeddings = []
            similarity_scores = []
            for i, idx in enumerate(indices):
                if idx == -1:  # FAISS returns -1 for missing results
                    continue
                docstore_id = vectorstore.index_to_docstore_id[idx]
                doc = vectorstore.docstore.search(docstore_id)
                search_results.append(doc)
                # Reconstruct embedding from FAISS index - O(1) per index
                embedding = vectorstore.index.reconstruct(int(idx))
                search_embeddings.append(embedding.tolist())
                # Convert L2 squared distance to cosine similarity
                # For normalized vectors: cosine_sim = 1 - (L2² / 2)
                cosine_sim = 1 - (float(distances[i]) / 2)
                similarity_scores.append(cosine_sim)

            if similarity_scores[0] < 0.2:
                first_shortlist = [agent["name"] for agent in agent_cards]
            else:
                first_shortlist = [result.metadata["name"] for result in search_results]
            logger.info(f"First shortlist of agents: {first_shortlist}")

            if conversation_history is None or len(conversation_history) == 0:
                second_shortlist = first_shortlist[0:10]
            else:
                # Re-rank the first shortlist using the conversation history and cached embeddings
                second_shortlist = self._rerank_agents(
                    search_results,
                    search_embeddings,
                    message,
                    conversation_history,
                    k=10,
                )
            logger.info(f"Second shortlist of agents: {second_shortlist}")

            # Filter agent cards to only include shortlisted ones
            shortlisted_agent_cards = []
            for agent_card in agent_cards:
                if agent_card.get("name") in second_shortlist:
                    shortlisted_agent_cards.append(agent_card)

            return (
                first_shortlist,
                similarity_scores,
                second_shortlist,
                shortlisted_agent_cards,
            )

        except Exception as e:
            error_msg = f"Semantic search failed: {str(e)}"
            logger.error(error_msg)
            raise RoutingEngineError(error_msg) from e

    def _llm_route(
        self,
        message: str,
        conversation_history: List[Dict[str, str]],
        agent_cards: List[Dict[str, Any]],
    ) -> RouterOutput:
        """
        Use LLM to make final agent selection from shortlisted agents.

        Args:
            message: User's query message
            agent_cards: Shortlisted agent cards

        Returns:
            RouterOutput with selected agent information

        Raises:
            RoutingEngineError: If LLM routing fails
        """
        try:
            system_prompt = """You are an agent router. Your job is to route a user's request to the appropriate agent.
INSTRUCTIONS: 
1. You will be given a user's request along with the current conversation history of the user eith multiple different agents.
2. You will also be given a list of agent ids along with their capabilities.
3. You must use this list to determine which agent is appropriate to serve the current user's request in the context of teh conversation history.
4. You must return agent_id of the agent which should be used to serve the request.
5. Remember you have to select an agent to serve the current user request and not one of the requests they made in the past."""

            user_prompt = """List of agents:  {agent_cards}.
Conversation history: {conversation_history}.
User's request: {message}."""

            prompt_template = ChatPromptTemplate.from_messages(
                [SystemMessage(content=system_prompt), ("human", user_prompt)]
            )

            # Prepare agent cards as JSON string
            agent_cards_str = json.dumps(agent_cards, indent=2) + "\n"

            # Create and invoke prompt
            prompt = prompt_template.invoke(
                {
                    "message": message,
                    "conversation_history": conversation_history,
                    "agent_cards": agent_cards_str,
                }
            )

            response = self.llm.invoke(prompt)

            if not isinstance(response, RouterOutput):
                raise RoutingEngineError("LLM response is not of type RouterOutput")

            logger.info(f"LLM selected agent: {response.agent_name}")
            return response

        except Exception as e:
            logger.error(f"Error invoking LLM: {e}")
            logger.error(f"Prompt sent: {prompt}")
            raise RoutingEngineError(f"LLM routing failed: {str(e)}") from e


# Convenience function for backward compatibility
def router(
    message: str,
    conversation_history: List[Dict[str, str]],
    agent_cards: List[Dict[str, Any]],
    vectorstore: FAISS,
) -> Tuple[List[str], List[float], List[str], RouterOutput]:
    """
    Route a user query to the most appropriate agent.

    This is a convenience function that maintains backward compatibility
    with the existing router interface.

    Args:
        message: User's query message
        conversation_history: User's conversation history in the current session
        agent_cards: List of available agent card dictionaries
        vectorstore: FAISS vector store for similarity search

    Returns:
        Tuple of (shortlisted_agents, router_output)
    """
    routing_engine = RoutingEngine()
    return routing_engine.route_query(
        message, conversation_history, agent_cards, vectorstore
    )
