export async function fetchApi(path, opts = {}) {
  const res = await fetch(`/api${path}`, opts);
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}
