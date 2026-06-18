export function connectSSE(path, { onMessage, onError, onOpen } = {}) {
  const source = new EventSource(`/api${path}`);
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
