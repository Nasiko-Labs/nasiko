from fastapi import FastAPI
from starlette.middleware.sessions import SessionMiddleware
from contextlib import asynccontextmanager
from motor.motor_asyncio import AsyncIOMotorClient
from app.pkg.config.config import settings
from app.repository.repository import Repository
from app.service.service import Service
from app.api.handlers import HandlerFactory
from app.api.routes.router import create_router
import asyncio
import logging
import secrets

from starlette.requests import Request
from starlette.responses import Response

from app.pkg.platform_logging import (
    setup_platform_logging,
    start_log_worker,
    stop_log_worker,
)

# Setup logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
    handlers=[logging.StreamHandler()],
)

# Silence pymongo debug logs
logging.getLogger("pymongo").setLevel(logging.WARNING)

# Set specific logger levels for app modules
logging.getLogger("app").setLevel(logging.INFO)
logging.getLogger("app.api.handlers").setLevel(logging.INFO)

logger = logging.getLogger(__name__)
logger.info("Logger initialized successfully")


def init_db():
    global client
    client = AsyncIOMotorClient(settings.MONGO_URI, uuidRepresentation="standard")
    return client[settings.MONGO_DB]


@asynccontextmanager
async def lifespan(app: FastAPI):
    logger.info("starting application...")
    db = init_db()
    repo = Repository(db, logger)

    # Initialize database collections and indexes
    await repo.ensure_collections()

    service = Service(repo, logger)
    handlers = HandlerFactory(service, logger, {})

    log_queue: asyncio.Queue = asyncio.Queue()
    await start_log_worker(repo.platform_logs, log_queue)
    setup_platform_logging(log_queue, service_name="nasiko-backend")
    seeded = await service.platform_logs.seed_if_empty()
    if seeded:
        logger.info(f"Seeded {seeded} sample platform log entries")

    # Initialize search service
    try:
        await handlers.search.initialize_search()
        logger.info("Search service initialized successfully")
    except Exception as e:
        logger.error(f"Failed to initialize search service: {e}")

    app.include_router(create_router(handlers, logger), prefix="/api/v1")
    app.state.platform_log_queue = log_queue

    yield
    # Shutdown
    await stop_log_worker()
    logger.info("shutting down application...")


app = FastAPI(
    title="Nasiko API",
    description="Nasiko Agent Registry with observability",
    version="0.0.1",
    docs_url="/docs",
    redoc_url="/redoc",
    openapi_url="/openapi.json",
    lifespan=lifespan,
)

# Add Session middleware (must be before CORS for cookies to work)
app.add_middleware(
    SessionMiddleware,
    secret_key=getattr(settings, "SESSION_SECRET_KEY", secrets.token_urlsafe(32)),
    max_age=86400,  # 24 hours
    same_site="lax",
    https_only=False,  # Set to True in production with HTTPS
)

# CORS is handled by Kong gateway, removed service-level CORS to avoid conflicts


@app.middleware("http")
async def platform_request_logging_middleware(request: Request, call_next):
    """Record API requests as platform INFO logs (skips health/docs noise)."""
    path = request.url.path
    skip = path in ("/api/v1/healthcheck", "/docs", "/redoc", "/openapi.json")
    response: Response = await call_next(request)
    if not skip and path.startswith("/api/"):
        queue = getattr(request.app.state, "platform_log_queue", None)
        if queue is not None:
            entry = {
                "level": "ERROR" if response.status_code >= 500 else "INFO",
                "message": f"{request.method} {path} -> {response.status_code}",
                "service": "nasiko-backend",
                "logger": "app.http",
            }
            await queue.put(entry)
    return response

# # Add explicit OPTIONS handler for preflight requests
# @app.options("/{full_path:path}")
# async def options_handler():
#     return {}
