"""
Chat History Service
Receives chat logs from Kong plugin and stores them in MongoDB
"""

from fastapi import FastAPI, HTTPException, Request
from pydantic import BaseModel
from typing import List, Dict, Any, Optional
import json
import logging
from datetime import datetime, timezone
from motor.motor_asyncio import AsyncIOMotorClient
import os

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = FastAPI(title="Chat History Service", version="1.0.0")

# MongoDB connection
MONGO_URL = os.getenv("MONGO_URL", "mongodb://mongodb:27017")
DB_NAME = os.getenv("CHAT_DB_NAME", "nasiko")
client = AsyncIOMotorClient(MONGO_URL)
db = client[DB_NAME]
chat_history_collection = db["chat-history"]

class ChatMessage(BaseModel):
    session_id: str
    message_id: str
    role: str  # "user" or "assistant"
    content: str
    timestamp: datetime
    context_id: Optional[str] = None
    task_id: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = {}

class ChatLogRequest(BaseModel):
    request_data: Dict[str, Any]
    response_data: Dict[str, Any]
    timestamp: Optional[datetime] = None

@app.on_event("startup")
async def startup_db():
    """Create indexes for chat history collection"""
    try:
        # Create indexes for efficient queries
        await chat_history_collection.create_index("session_id")
        await chat_history_collection.create_index("timestamp")
        await chat_history_collection.create_index([("session_id", 1), ("timestamp", 1)])
        logger.info("Chat history service started successfully")
    except Exception as e:
        logger.error(f"Failed to initialize database: {str(e)}")

def extract_user_message(request_data: Dict[str, Any]) -> Optional[ChatMessage]:
    """Extract user message from JSONRPC request"""
    try:
        session_id = request_data.get("id")
        if not session_id:
            return None
            
        params = request_data.get("params", {})
        message = params.get("message", {})
        
        if message.get("role") != "user":
            return None
            
        # Extract text from parts
        parts = message.get("parts", [])
        content_parts = []
        for part in parts:
            if part.get("kind") == "text":
                content_parts.append(part.get("text", ""))
        
        content = " ".join(content_parts).strip()
        if not content:
            return None
            
        return ChatMessage(
            session_id=session_id,
            message_id=message.get("messageId", ""),
            role="user",
            content=content,
            timestamp=datetime.now(timezone.utc),
            metadata={"request_method": request_data.get("method", "")}
        )
    except Exception as e:
        logger.error(f"Error extracting user message: {str(e)}")
        return None

def extract_assistant_message(response_data: Dict[str, Any]) -> Optional[ChatMessage]:
    """Extract assistant message from JSONRPC response"""
    try:
        session_id = response_data.get("id")
        if not session_id:
            return None
            
        result = response_data.get("result", {})
        
        # Extract text from artifacts
        artifacts = result.get("artifacts", [])
        content_parts = []
        
        for artifact in artifacts:
            parts = artifact.get("parts", [])
            for part in parts:
                if part.get("kind") == "text":
                    content_parts.append(part.get("text", ""))
        
        content = " ".join(content_parts).strip()
        if not content:
            return None
            
        return ChatMessage(
            session_id=session_id,
            message_id="",  # Response doesn't have messageId
            role="assistant",
            content=content,
            timestamp=datetime.now(timezone.utc),
            context_id=result.get("contextId"),
            task_id=result.get("id"),
            metadata={
                "status": result.get("status", {}),
                "kind": result.get("kind", "")
            }
        )
    except Exception as e:
        logger.error(f"Error extracting assistant message: {str(e)}")
        return None

@app.post("/log-chat")
async def log_chat_interaction(log_request: ChatLogRequest):
    """Log chat interaction from Kong plugin"""
    try:
        messages_to_store = []
        
        # Extract user message from request
        user_message = extract_user_message(log_request.request_data)
        if user_message:
            messages_to_store.append(user_message.dict())
            
        # Extract assistant message from response
        assistant_message = extract_assistant_message(log_request.response_data)
        if assistant_message:
            messages_to_store.append(assistant_message.dict())
        
        # Store messages in MongoDB
        if messages_to_store:
            await chat_history_collection.insert_many(messages_to_store)
            logger.info(f"Stored {len(messages_to_store)} chat messages for session {user_message.session_id if user_message else 'unknown'}")
            
            return {
                "success": True,
                "message": f"Logged {len(messages_to_store)} messages",
                "session_id": user_message.session_id if user_message else None
            }
        else:
            return {
                "success": True,
                "message": "No messages to log",
                "session_id": None
            }
            
    except Exception as e:
        logger.error(f"Error logging chat interaction: {str(e)}")
        raise HTTPException(status_code=500, detail=f"Failed to log chat: {str(e)}")

@app.get("/chat-history/{session_id}")
async def get_chat_history(session_id: str, limit: int = 50):
    """Retrieve chat history for a session"""
    try:
        cursor = chat_history_collection.find(
            {"session_id": session_id}
        ).sort("timestamp", 1).limit(limit)
        
        messages = await cursor.to_list(length=None)
        
        # Convert ObjectId to string for JSON serialization
        for message in messages:
            if "_id" in message:
                message["_id"] = str(message["_id"])
                
        return {
            "session_id": session_id,
            "messages": messages,
            "count": len(messages)
        }
        
    except Exception as e:
        logger.error(f"Error retrieving chat history: {str(e)}")
        raise HTTPException(status_code=500, detail=f"Failed to retrieve chat history: {str(e)}")

@app.get("/health")
async def health_check():
    """Health check endpoint"""
    try:
        # Test database connection
        await db.list_collection_names()
        return {"status": "healthy", "service": "chat-history-service"}
    except Exception as e:
        raise HTTPException(status_code=503, detail=f"Service unhealthy: {str(e)}")

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8002)