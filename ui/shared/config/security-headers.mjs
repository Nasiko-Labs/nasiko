/** Static SPA headers, shared by local verification and the deployment contract. */
export function securityHeaders({ development = false } = {}) {
  return {
    "Content-Security-Policy": [
      "default-src 'self'",
      "script-src 'self'" + (development ? " 'unsafe-inline'" : ""),
      "style-src 'self' 'unsafe-inline'",
      "img-src 'self' data: blob:",
      "font-src 'self'",
      "connect-src 'self'" + (development ? " ws://localhost:* ws://127.0.0.1:*" : ""),
      "frame-ancestors 'none'",
      "object-src 'none'",
      "base-uri 'self'",
      "form-action 'self'",
    ].join("; "),
    "X-Content-Type-Options": "nosniff",
    "Referrer-Policy": "strict-origin-when-cross-origin",
    "X-Frame-Options": "DENY",
    "Permissions-Policy": "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
    "Cross-Origin-Opener-Policy": "same-origin",
  };
}
