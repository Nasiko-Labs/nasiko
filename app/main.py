from fastapi import FastAPI
from starlette.middleware.sessions import SessionMiddleware
from contextlib import asynccontextmanager
from motor.motor_asyncio import AsyncIOMotorClient
from app.pkg.config.config import settings
from app.repository.repository import Repository
from app.service.service import Service
from app.api.handlers import HandlerFactory
from app.api.routes.router import create_router
from app.utils.log_buffer import install_platform_log_handler
import logging
import os
import secrets
import time
import uuid

from starlette.requests import Request

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
install_platform_log_handler()

logger = logging.getLogger(__name__)
logger.info("Logger initialized successfully")

_ACCESS_LOG_SKIP_PATHS = {
    "/api/v1/healthcheck",
    "/docs",
    "/redoc",
    "/openapi.json",
}


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

    # Initialize search service
    try:
        await handlers.search.initialize_search()
        logger.info("Search service initialized successfully")
    except Exception as e:
        logger.error(f"Failed to initialize search service: {e}")

    app.include_router(create_router(handlers, logger), prefix="/api/v1")
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
async def platform_access_log_middleware(request: Request, call_next):
    """Capture useful API request logs for the platform log dashboard."""

    started_at = time.perf_counter()
    request_id = request.headers.get("x-request-id") or f"req_{uuid.uuid4().hex[:10]}"

    try:
        response = await call_next(request)
    except Exception:
        latency_ms = _elapsed_ms(started_at)
        if _should_capture_access_log(request.url.path):
            logger.exception(
                f"{request.method} {request.url.path} -> 500",
                extra=_access_log_extra(
                    request=request,
                    request_id=request_id,
                    status_code=500,
                    latency_ms=latency_ms,
                ),
            )
        raise

    latency_ms = _elapsed_ms(started_at)
    response.headers.setdefault("X-Request-ID", request_id)

    if _should_capture_access_log(request.url.path):
        status_code = response.status_code
        logger.log(
            _access_log_level(status_code),
            f"{request.method} {request.url.path} -> {status_code}",
            extra=_access_log_extra(
                request=request,
                request_id=request_id,
                status_code=status_code,
                latency_ms=latency_ms,
            ),
        )

    return response


def _should_capture_access_log(path: str) -> bool:
    if path in _ACCESS_LOG_SKIP_PATHS:
        return False
    return path.startswith("/api/") and not path.startswith("/api/v1/platform/logs")


def _elapsed_ms(started_at: float) -> int:
    return max(0, round((time.perf_counter() - started_at) * 1000))


def _access_log_level(status_code: int) -> int:
    if status_code >= 500:
        return logging.ERROR
    if status_code >= 400:
        return logging.WARNING
    return logging.INFO


def _access_log_extra(
    *,
    request: Request,
    request_id: str,
    status_code: int,
    latency_ms: int,
):
    route = f"{request.method} {request.url.path}"
    return {
        "service": "nasiko-backend",
        "route": route,
        "trace_id": request.headers.get("x-trace-id") or request_id,
        "request_id": request_id,
        "latency_ms": latency_ms,
        "status_code": status_code,
        "pod": os.getenv("HOSTNAME", "nasiko-backend"),
        "commit": os.getenv("GIT_SHA", "runtime"),
        "source": "platform_access_log_middleware",
    }

# # Add explicit OPTIONS handler for preflight requests
# @app.options("/{full_path:path}")
# async def options_handler():
#     return {}
