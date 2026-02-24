"""
Vector store service for agent selection and similarity search.
"""

import logging
from typing import List, Dict, Tuple, Optional
from functools import lru_cache

from langchain_community.vectorstores import FAISS
from langchain_openai import OpenAIEmbeddings

from router.src.config import settings

logger = logging.getLogger(__name__)


class VectorStoreError(Exception):
    """Custom exception for vector store errors."""
    pass


class VectorStoreService:
    """Service for managing vector stores and similarity search."""
    
    def __init__(self):
        self.embeddings = self._create_embeddings()
        self._store_cache: Optional[FAISS] = None
        self._cache_hash: Optional[str] = None
    
    def _create_embeddings(self) -> OpenAIEmbeddings:
        """Create OpenAI embeddings instance."""
        if not settings.OPENAI_API_KEY:
            raise VectorStoreError("OpenAI API key is required for embeddings")
        
        return OpenAIEmbeddings(
            model=settings.EMBEDDING_MODEL,
            openai_api_key=settings.OPENAI_API_KEY
        )
    
    def create_vector_store(self, agent_cards: List[Dict[str, str]], use_cache: bool = True) -> FAISS:
        """
        Create a vector store from agent cards.
        
        Args:
            agent_cards: List of agent card dictionaries
            use_cache: Whether to use cached vector store if available
            
        Returns:
            FAISS vector store instance
            
        Raises:
            VectorStoreError: If creation fails
        """
        # Create hash of agent cards for cache validation
        cards_hash = self._hash_agent_cards(agent_cards)
        
        if use_cache and self._is_cache_valid(cards_hash):
            logger.info("Using cached vector store")
            return self._store_cache
        
        texts, metadatas = self._prepare_data(agent_cards)
        
        if not texts or not metadatas:
            raise VectorStoreError("No valid agent cards found with description and name")
        
        try:
            logger.info(f"Creating vector store with {len(texts)} agent descriptions")
            vectorstore = FAISS.from_texts(texts, embedding=self.embeddings, metadatas=metadatas)
            
            # Update cache
            self._store_cache = vectorstore
            self._cache_hash = cards_hash
            
            return vectorstore
            
        except Exception as e:
            error_msg = f"Failed to create vector store: {e}"
            logger.error(error_msg)
            raise VectorStoreError(error_msg) from e
    
    def _prepare_data(self, agent_cards: List[Dict[str, str]]) -> Tuple[List[str], List[Dict[str, str]]]:
        """
        Prepare texts and metadata for vector store creation.
        
        Args:
            agent_cards: List of agent card dictionaries
            
        Returns:
            Tuple of (texts, metadatas)
        """
        texts = []
        metadatas = []
        
        for agent_card in agent_cards:
            try:
                description = agent_card.get("description", "")
                name = agent_card.get("name", "")
                
                if not description or not name:
                    logger.warning(f"Agent card missing description or name: {agent_card}")
                    continue
                
                texts.append(description)
                metadatas.append({"name": name})
                
            except Exception as e:
                logger.error(f"Error processing agent card {agent_card}: {e}")
                continue
        
        return texts, metadatas
    
    def _hash_agent_cards(self, agent_cards: List[Dict[str, str]]) -> str:
        """Create a hash of agent cards for cache validation."""
        import hashlib
        import json
        
        # Sort cards by name for consistent hashing
        sorted_cards = sorted(agent_cards, key=lambda x: x.get("name", ""))
        cards_json = json.dumps(sorted_cards, sort_keys=True)
        return hashlib.md5(cards_json.encode()).hexdigest()
    
    def _is_cache_valid(self, cards_hash: str) -> bool:
        """Check if cached vector store is still valid."""
        return (self._store_cache is not None and 
                self._cache_hash is not None and 
                self._cache_hash == cards_hash)
    
    def similarity_search(self, vectorstore: FAISS, query: str, k: int = 5) -> List[Dict[str, str]]:
        """
        Perform similarity search on the vector store.
        
        Args:
            vectorstore: FAISS vector store instance
            query: Query text
            k: Number of results to return
            
        Returns:
            List of similar agent metadata
        """
        try:
            results = vectorstore.similarity_search_with_score(query, k=k)
            
            # Extract metadata and scores
            similar_agents = []
            for doc, score in results:
                metadata = doc.metadata.copy()
                metadata['similarity_score'] = score
                similar_agents.append(metadata)
            
            logger.info(f"Found {len(similar_agents)} similar agents for query: {query[:100]}...")
            return similar_agents
            
        except Exception as e:
            error_msg = f"Similarity search failed: {e}"
            logger.error(error_msg)
            raise VectorStoreError(error_msg) from e
    
    def clear_cache(self) -> None:
        """Clear the vector store cache."""
        self._store_cache = None
        self._cache_hash = None
        logger.info("Vector store cache cleared")