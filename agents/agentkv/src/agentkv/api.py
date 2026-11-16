"""Authenticated control-plane API. Owner identity comes only from server configuration."""
import hmac
import httpx
from contextlib import asynccontextmanager

from fastapi import Depends, FastAPI, HTTPException
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

from agentkv.control import Conflict, Controller, Event, Store
from agentkv.nasiko import NasikoBridge


def create_app(tokens: dict[str, str], store: Store, estimator=None,
               nasiko: NasikoBridge | None = None) -> FastAPI:
    if not tokens or any(len(k) < 32 or not v for k, v in tokens.items()):
        raise ValueError("configure a strong token and trusted owner for each principal")
    controller = Controller(store, estimator)
    bearer = HTTPBearer(auto_error=False)

    async def owner(credentials: HTTPAuthorizationCredentials | None = Depends(bearer)):
        if credentials and credentials.scheme.lower() == "bearer":
            for token, principal in tokens.items():
                if hmac.compare_digest(credentials.credentials.encode(), token.encode()):
                    return principal
        raise HTTPException(401, "Authentication required")

    @asynccontextmanager
    async def lifespan(app):
        yield
        if nasiko:
            await nasiko.close()
        store.close()

    app = FastAPI(title="AgentKV", lifespan=lifespan)

    @app.get("/health")
    def health():
        return {"status": "ok", "engine_adapter": "unconnected"}

    @app.post("/events")
    def events(event: Event, principal: str = Depends(owner)):
        try:
            accepted = store.ingest(principal, event)
        except Conflict as exc:
            raise HTTPException(409, str(exc)) from None
        return {"accepted": accepted, "revision": event.revision}

    @app.post("/flows/{flow_id}/decide")
    async def decide(flow_id: str, principal: str = Depends(owner)):
        result = await controller.decide(principal, flow_id)
        if result is None:
            raise HTTPException(404, "Flow not found")
        return result

    @app.get("/flows/{flow_id}/decisions")
    def decisions(flow_id: str, principal: str = Depends(owner)):
        if store.snapshot(principal, flow_id) is None:
            raise HTTPException(404, "Flow not found")
        return {"decisions": store.history(principal, flow_id)}

    @app.post("/flows/{flow_id}/sync")
    async def sync(flow_id: str, principal: str = Depends(owner)):
        if nasiko is None:
            raise HTTPException(503, "Nasiko bridge is not configured")
        if principal != nasiko.owner:
            raise HTTPException(404, "Flow not found")
        try:
            return {"accepted": await nasiko.sync(flow_id)}
        except httpx.HTTPError:
            raise HTTPException(502, "Nasiko request failed") from None
        except (ValueError, KeyError, TypeError, Conflict):
            raise HTTPException(409, "Nasiko snapshot rejected") from None

    @app.get("/metrics")
    def metrics(principal: str = Depends(owner)):
        return store.metrics(principal)

    return app
