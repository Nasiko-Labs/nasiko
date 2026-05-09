from fastapi import APIRouter, Request
from fastapi.responses import HTMLResponse
from pathlib import Path

router = APIRouter()

@router.get("/", response_class=HTMLResponse)
@router.get("/dashboard", response_class=HTMLResponse)
async def dashboard(request: Request):
    html = Path("templates/dashboard.html").read_text()
    return HTMLResponse(content=html)