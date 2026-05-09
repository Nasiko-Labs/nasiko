from fastapi.responses import HTMLResponse


def dashboard_html() -> HTMLResponse:
    return HTMLResponse(
        """
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Nasiko Request Manager</title>
  <style>
    :root { --ink:#17211c; --muted:#5f6f66; --panel:#fffaf0; --line:#dfd3bd; --accent:#0e7c5f; --warn:#b65324; }
    body { margin:0; font-family: ui-serif, Georgia, serif; color:var(--ink); background:linear-gradient(135deg,#f6eedf,#d9eadf); }
    main { width:min(1180px, calc(100vw - 32px)); margin:32px auto; }
    h1 { font-size:42px; margin:0 0 8px; letter-spacing:-0.03em; }
    p { color:var(--muted); }
    .grid { display:grid; grid-template-columns:repeat(auto-fit,minmax(220px,1fr)); gap:16px; margin:24px 0; }
    .card { background:rgba(255,250,240,.84); border:1px solid var(--line); border-radius:22px; padding:18px; box-shadow:0 18px 50px rgba(45,35,20,.08); }
    .metric { font-size:34px; font-weight:700; }
    table { width:100%; border-collapse:collapse; background:rgba(255,250,240,.84); border-radius:18px; overflow:hidden; }
    th,td { padding:12px; border-bottom:1px solid var(--line); text-align:left; }
    th { color:var(--muted); font-size:12px; text-transform:uppercase; letter-spacing:.08em; }
    .pill { display:inline-block; border-radius:999px; padding:4px 9px; background:#dff2e7; color:var(--accent); font-size:12px; }
    .bad { background:#ffe1d0; color:var(--warn); }
  </style>
</head>
<body>
<main>
  <h1>Nasiko Request Manager</h1>
  <p>Live cache, queue, rate-limit, and circuit-breaker view for agent traffic.</p>
  <section class="grid" id="cards"></section>
  <table>
    <thead><tr><th>Agent</th><th>Active</th><th>Queued</th><th>Hit Rate</th><th>P95 Latency</th><th>P95 Queue</th><th>Circuit</th></tr></thead>
    <tbody id="agents"></tbody>
  </table>
</main>
<script>
async function refresh() {
  const res = await fetch('/control/stats');
  const data = await res.json();
  const total = data.cache_hits + data.cache_misses;
  const hitRate = total ? Math.round((data.cache_hits / total) * 100) : 0;
  document.getElementById('cards').innerHTML = [
    ['Status', data.status],
    ['Cache Hit Rate', hitRate + '%'],
    ['Active Requests', data.active_requests],
    ['Upstream Errors', data.upstream_errors],
    ['Queue Timeouts', data.queue_timeouts],
  ].map(([k,v]) => `<article class="card"><p>${k}</p><div class="metric">${v}</div></article>`).join('');
  document.getElementById('agents').innerHTML = data.agents.map(agent => {
    const total = agent.cache_hits + agent.cache_misses;
    const rate = total ? Math.round((agent.cache_hits / total) * 100) : 0;
    const cls = agent.circuit_state === 'closed' ? 'pill' : 'pill bad';
    return `<tr><td>${agent.agent_id}</td><td>${agent.active_requests}</td><td>${agent.queued_requests}</td><td>${rate}%</td><td>${agent.p95_latency_ms}ms</td><td>${agent.p95_queue_wait_ms}ms</td><td><span class="${cls}">${agent.circuit_state}</span></td></tr>`;
  }).join('');
}
refresh(); setInterval(refresh, 2000);
</script>
</body>
</html>
        """
    )
