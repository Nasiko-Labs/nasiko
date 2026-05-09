from fastapi import FastAPI

from request_manager.settings import get_settings

settings = get_settings()

app = FastAPI(
    title="Nasiko Request Manager",
    version="0.1.0",
    description="Traffic-control layer for Nasiko agent requests.",
)


@app.get("/health")
async def health() -> dict[str, object]:
    return {
        "status": "starting",
        "service": settings.service_name,
        "redis_available": False,
        "circuits": {},
    }
