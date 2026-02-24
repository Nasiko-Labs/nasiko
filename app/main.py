from fastapi import FastAPI
from starlette.middleware.sessions import SessionMiddleware
from contextlib import asynccontextmanager
from motor.motor_asyncio import AsyncIOMotorClient
from app.pkg.config.config import settings
from app.repository.repository import Repository
from app.service.service import Service
from app.api.handlers import HandlerFactory
from app.api.routes.router import create_router
import logging
import secrets

# Setup logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s',
    handlers=[
        logging.StreamHandler()
    ]
)

# Silence pymongo debug logs
logging.getLogger('pymongo').setLevel(logging.WARNING)

# Set specific logger levels for app modules
logging.getLogger('app').setLevel(logging.INFO)
logging.getLogger('app.api.handlers').setLevel(logging.INFO)

logger = logging.getLogger(__name__)
logger.info("Logger initialized successfully")

def init_db():
    global client
    client = AsyncIOMotorClient(settings.MONGO_URI, uuidRepresentation='standard')
    return client[settings.MONGO_DB]

@asynccontextmanager
async def lifespan(app : FastAPI):
    logger.info("starting application...")
    db = init_db()
    repo = Repository(db, logger)
    
    # Initialize database collections and indexes
    await repo.ensure_collections()
    
    service = Service(repo, logger)
    handlers = HandlerFactory(service, logger, {})
    
    # Initialize search service
    try:
        await handlers.search.initialize_search()
        logger.info("Search service initialized successfully")
    except Exception as e:
        logger.error(f"Failed to initialize search service: {e}")
    
    app.include_router(
       create_router(handlers, logger), prefix="/api/v1"
    )
    yield
    # Shutdown
    logger.info("shutting down application...")

app = FastAPI(
    title="Nasiko API",
    description="Nasiko Agent Registry with observability",
    version="0.0.1",
    docs_url="/docs",
    redoc_url="/redoc",
    openapi_url="/openapi.json",
    lifespan=lifespan
)

# Add Session middleware (must be before CORS for cookies to work)
app.add_middleware(
    SessionMiddleware, 
    secret_key=getattr(settings, 'SESSION_SECRET_KEY', secrets.token_urlsafe(32)),
    max_age=86400,  # 24 hours
    same_site='lax',
    https_only=False  # Set to True in production with HTTPS
)

# CORS is handled by Kong gateway, removed service-level CORS to avoid conflicts

# # Add explicit OPTIONS handler for preflight requests
# @app.options("/{full_path:path}")
# async def options_handler():
#     return {}
