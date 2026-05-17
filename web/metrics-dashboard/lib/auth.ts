/**
 * Auth helpers for Route Handlers — forward Nasiko session JWT from the browser.
 */

export function getBearerFromAuthorizationHeader(
  authorization: string | null,
): string | null {
  if (!authorization) {
    return null;
  }
  const match = /^Bearer\s+(.+)$/i.exec(authorization.trim());
  return match?.[1] ?? null;
}

export function getAuthorizationHeader(request: Request): string | null {
  return request.headers.get("authorization");
}

/** Returns `Bearer <token>` for nasikoFetch, or null if missing/invalid. */
export function getBearerAuthForNasiko(request: Request): string | null {
  const header = getAuthorizationHeader(request);
  if (!header?.startsWith("Bearer ")) {
    return null;
  }
  const token = getBearerFromAuthorizationHeader(header);
  if (!token) {
    return null;
  }
  return `Bearer ${token}`;
}

export function unauthorizedResponse(message = "Unauthorized"): Response {
  return Response.json({ error: message }, { status: 401 });
}
