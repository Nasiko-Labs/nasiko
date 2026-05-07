"""Kong Admin API registrar for MCP Bridge services and routes."""

from __future__ import annotations

import httpx


class KongRegistrationError(Exception):
    """Raised when Kong Admin API returns a non-2xx response."""


class KongRegistrar:
    """Register an MCP bridge as a Kong service + route.

    Only performs two POST calls — NEVER touches existing Kong resources,
    never calls DELETE or PATCH.
    """

    def __init__(self, admin_url: str) -> None:
        self.admin_url = admin_url.rstrip("/")

    def register(self, artifact_id: str, port: int) -> tuple[str, str]:
        """Register a service and route in Kong for the given artifact.

        Returns:
            (kong_service_id, kong_route_id)

        Raises:
            KongRegistrationError: If either Kong API call returns non-2xx.
        """
        service_name = f"mcp-{artifact_id}"

        # ------------------------------------------------------------------
        # Step 1 — Create Kong service
        # ------------------------------------------------------------------
        with httpx.Client(timeout=5.0) as client:
            svc_resp = client.post(
                f"{self.admin_url}/services",
                json={
                    "name": service_name,
                    "url": f"http://localhost:{port}",
                },
            )

        if not (200 <= svc_resp.status_code < 300):
            raise KongRegistrationError(
                f"Kong service creation failed "
                f"(HTTP {svc_resp.status_code}): {svc_resp.text}"
            )

        kong_service_id: str = svc_resp.json()["id"]

        # ------------------------------------------------------------------
        # Step 2 — Create Kong route on that service
        # ------------------------------------------------------------------
        with httpx.Client(timeout=5.0) as client:
            route_resp = client.post(
                f"{self.admin_url}/services/{service_name}/routes",
                json={
                    "name": f"mcp-route-{artifact_id}",
                    "paths": [f"/mcp/{artifact_id}"],
                },
            )

        if not (200 <= route_resp.status_code < 300):
            raise KongRegistrationError(
                f"Kong route creation failed "
                f"(HTTP {route_resp.status_code}): {route_resp.text}"
            )

        kong_route_id: str = route_resp.json()["id"]

        return kong_service_id, kong_route_id
