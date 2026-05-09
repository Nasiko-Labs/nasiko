import logging
import requests

from src.config import settings

logger = logging.getLogger(__name__)

_KONG = settings.KONG_ADMIN_URL


def _upsert_service(name: str, url: str) -> bool:
    data = {
        "name": name,
        "url": url,
        "connect_timeout": 60000,
        "write_timeout": 300000,
        "read_timeout": 300000,
        "retries": 3,
        "protocol": "http",
    }
    try:
        resp = requests.get(f"{_KONG}/services/{name}", timeout=5)
        if resp.status_code == 200:
            requests.patch(f"{_KONG}/services/{name}", json=data, timeout=10)
        else:
            requests.post(f"{_KONG}/services", json=data, timeout=10)
        return True
    except requests.RequestException as exc:
        logger.error(f"kong upsert_service {name} failed: {exc}")
        return False


def _upsert_route(service_name: str, route_name: str, paths: list[str], strip_path: bool = False) -> bool:
    route_data = {
        "name": route_name,
        "paths": paths,
        "methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"],
        "strip_path": strip_path,
        "preserve_host": False,
    }
    try:
        resp = requests.get(f"{_KONG}/routes/{route_name}", timeout=5)
        if resp.status_code == 200:
            requests.patch(f"{_KONG}/routes/{route_name}", json=route_data, timeout=10)
        else:
            route_data["service"] = {"name": service_name}
            requests.post(f"{_KONG}/services/{service_name}/routes", json=route_data, timeout=10)
        return True
    except requests.RequestException as exc:
        logger.error(f"kong upsert_route {route_name} failed: {exc}")
        return False


def register_request_layer() -> None:
    """
    Register request-layer as the upstream for /agents/* and /router/* paths.
    Called at startup so Kong routes all agent traffic through this service.
    """
    layer_url = "http://nasiko-request-layer:8090"

    ok = _upsert_service("nasiko-request-layer", layer_url)
    if not ok:
        logger.warning("could not register request-layer service in Kong")
        return

    _upsert_route("nasiko-request-layer", "request-layer-agents-route", ["/agents"], strip_path=False)
    _upsert_route("nasiko-request-layer", "request-layer-router-route", ["/router"], strip_path=False)

    logger.info("request-layer registered with Kong successfully")


def add_upstream_target(upstream_name: str, target: str, weight: int = 100) -> bool:
    try:
        requests.post(
            f"{_KONG}/upstreams/{upstream_name}/targets",
            json={"target": target, "weight": weight},
            timeout=10,
        )
        return True
    except requests.RequestException as exc:
        logger.error(f"kong add_upstream_target {target} failed: {exc}")
        return False


def remove_upstream_target(upstream_name: str, target: str) -> bool:
    try:
        resp = requests.get(f"{_KONG}/upstreams/{upstream_name}/targets", timeout=5)
        if resp.status_code != 200:
            return False
        for t in resp.json().get("data", []):
            if t.get("target") == target:
                requests.delete(f"{_KONG}/upstreams/{upstream_name}/targets/{t['id']}", timeout=10)
                return True
        return False
    except requests.RequestException as exc:
        logger.error(f"kong remove_upstream_target {target} failed: {exc}")
        return False


def update_service_url(service_name: str, new_url: str) -> bool:
    try:
        requests.patch(f"{_KONG}/services/{service_name}", json={"url": new_url}, timeout=10)
        return True
    except requests.RequestException as exc:
        logger.error(f"kong update_service_url {service_name} failed: {exc}")
        return False
