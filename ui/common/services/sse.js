export function connectSSE(path, { onMessage, onError, onOpen } = {}) {
  // Same multi-tenant seam as apiFetch (see services/api.js): base from
  // window.nasikoConfig, credentialed when cross-origin.
  const base = window.nasikoConfig?.apiBase || "";
  const source = new EventSource(`${base}/api${path}`, base ? { withCredentials: true } : undefined);
  if (onOpen) source.addEventListener("open", onOpen);
  source.addEventListener("message", (e) => {
    try {
      const data = JSON.parse(e.data);
      if (onMessage) onMessage(data);
    } catch {
      if (onMessage) onMessage(e.data);
    }
  });
  source.addEventListener("error", (e) => {
    if (onError) onError(e);
  });
  return source;
}
