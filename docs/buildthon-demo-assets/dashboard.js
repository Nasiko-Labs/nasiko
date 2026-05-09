const ROUTER = "/api/router";
const KONG   = "/api/kong";
const ADMIN_KEY = "local-admin-key";
const POLL_MS = 3000;

const $ = (s) => document.getElementById(s);
let pollTimer = null;

async function api(url, opts = {}) {
  const headers = { ...(opts.headers || {}) };
  if (opts.admin) headers["X-Admin-API-Key"] = ADMIN_KEY;
  const res = await fetch(url, { ...opts, headers });
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  const ct = res.headers.get("content-type") || "";
  return ct.includes("json") ? res.json() : res.text();
}

function fmtDuration(s) {
  if (s < 60) return `${Math.round(s)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
  const h = Math.floor(s / 3600);
  return `${h}h ${Math.floor((s % 3600) / 60)}m`;
}

async function refresh() {
  try {
    const d = await api(`${ROUTER}/admin/stats/runtime`, { admin: true });
    $("statusDot").className = "status-dot online";
    $("statusText").textContent = "Online";
    $("uptime").textContent = `⏱ ${fmtDuration(d.uptime_seconds)}`;
    $("totalReqs").textContent = d.total_requests;
    $("hitRatio").textContent = `${(d.cache_hit_ratio * 100).toFixed(1)}%`;
    $("cacheHits").textContent = d.cache_hits_total;
    $("cacheMisses").textContent = d.cache_misses_total;
    $("semanticHits").textContent = d.semantic_hits_total;
    $("errors").textContent = d.errors_total;

    const c = d.cache || {};
    $("cTotalKeys").textContent = c.total_keys ?? 0;
    $("cActiveKeys").textContent = c.active_keys ?? 0;
    $("cStores").textContent = c.stores ?? 0;
    $("cTTL").textContent = `${c.ttl_seconds ?? 0}s`;
    $("cMaxSize").textContent = c.max_size ?? 0;
    $("cSemantic").textContent = c.semantic_enabled ? "✅ ON" : "❌ OFF";

    renderAgentTable(d);
  } catch (e) {
    $("statusDot").className = "status-dot offline";
    $("statusText").textContent = "Offline";
  }

  try {
    const m = await api(`${ROUTER}/metrics`);
    $("metricsRaw").textContent = m;
  } catch (_) {}
}

function renderAgentTable(d) {
  const rl = d.rate_limits || {};
  const lat = d.agent_latency || {};
  const tbody = $("agentTableBody");
  const agents = new Set([...Object.keys(rl), ...Object.keys(lat)]);

  if (agents.size === 0) {
    tbody.innerHTML = `<tr><td colspan="10" class="empty-row">No agent traffic yet</td></tr>`;
    return;
  }
  tbody.innerHTML = "";
  for (const aid of agents) {
    const r = rl[aid] || {};
    const l = lat[aid] || {};
    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td>${aid}</td>
      <td>${r.limit_rpm ?? "—"}</td>
      <td>${r.current_rpm ?? 0}</td>
      <td>${r.queue_depth ?? 0}</td>
      <td>${r.allowed ?? 0}</td>
      <td>${r.queued ?? 0}</td>
      <td>${r.rejected ?? 0}</td>
      <td>${l.avg_s ? (l.avg_s * 1000).toFixed(0) + "ms" : "—"}</td>
      <td>${l.p50_s ? (l.p50_s * 1000).toFixed(0) + "ms" : "—"}</td>
      <td>${l.p95_s ? (l.p95_s * 1000).toFixed(0) + "ms" : "—"}</td>`;
    tbody.appendChild(tr);
  }
}

async function runBenchmark() {
  const btn = $("runBenchmark");
  const body = $("benchmarkBody");
  btn.disabled = true;
  btn.textContent = "Running…";
  body.innerHTML = `<div class="bench-placeholder">⏳ Sending first request (cold)…</div>`;

  const token = $("authToken").value.trim() || "NASK_zzonHU_UdNhK1ZG-0GFGzA";
  const fd1 = new FormData(); fd1.append("session_id", "bench"); fd1.append("query", "translate hello world to French"); fd1.append("route", "a2a-translator");

  let firstMs, secondMs, resp1, resp2;
  try {
    const t1 = performance.now();
    const r1 = await fetch(`${KONG}/router`, { method: "POST", headers: { "Authorization": `Bearer ${token}` }, body: fd1 });
    firstMs = Math.round(performance.now() - t1);
    if (!r1.ok) throw new Error(`${r1.status} ${await r1.text()}`);
    resp1 = await r1.text();
  } catch (e) {
    body.innerHTML = `<div class="bench-placeholder" style="color:var(--red)">❌ First request failed: ${e.message}</div>`;
    btn.disabled = false;
    btn.textContent = "Run Benchmark";
    return;
  }

  body.innerHTML = `<div class="bench-placeholder">⏳ Sending second request (should be cached)…</div>`;

  try {
    const fd2 = new FormData(); fd2.append("session_id", "bench"); fd2.append("query", "translate hello world to French"); fd2.append("route", "a2a-translator");
    const t2 = performance.now();
    const r2 = await fetch(`${KONG}/router`, { method: "POST", headers: { "Authorization": `Bearer ${token}` }, body: fd2 });
    secondMs = Math.round(performance.now() - t2);
    if (!r2.ok) throw new Error(`${r2.status} ${await r2.text()}`);
    resp2 = await r2.text();
  } catch (e) {
    body.innerHTML = `<div class="bench-placeholder" style="color:var(--red)">❌ Second request failed: ${e.message}</div>`;
    btn.disabled = false;
    btn.textContent = "Run Benchmark";
    return;
  }

  const ratio = secondMs > 0 ? (firstMs / secondMs).toFixed(1) : "∞";
  const pass = parseFloat(ratio) >= 3;
  const maxMs = Math.max(firstMs, secondMs, 1);

  body.innerHTML = `
    <div class="bench-results">
      <div class="bench-row">
        <div class="bench-card">
          <div class="bench-card-label">Cold Call (no cache)</div>
          <div class="bench-card-value slow">${firstMs}ms</div>
          <div class="bench-bar-wrap"><div class="bench-bar first" style="width:${(firstMs / maxMs * 100)}%"></div></div>
        </div>
        <div class="bench-card">
          <div class="bench-card-label">Cached Call</div>
          <div class="bench-card-value fast">${secondMs}ms</div>
          <div class="bench-bar-wrap"><div class="bench-bar second" style="width:${(secondMs / maxMs * 100)}%"></div></div>
        </div>
        <div class="bench-card" style="display:flex;flex-direction:column;align-items:center;justify-content:center">
          <div class="bench-card-label">Speedup</div>
          <div class="speedup-badge ${pass ? 'pass' : 'fail'}">${pass ? '✅' : '⚠️'} ${ratio}x</div>
          <div style="font-size:11px;color:var(--text-muted);margin-top:6px">${pass ? 'PASS — meets 3x threshold' : 'Below 3x threshold'}</div>
        </div>
      </div>
    </div>`;

  btn.disabled = false;
  btn.textContent = "Run Benchmark";
  await refresh();
}

const CHECKS = [
  { id: "kong_health",      name: "Kong Gateway Health",     fn: () => api(`${KONG}/health`) },
  { id: "router_health",    name: "Router Health",           fn: () => api(`${ROUTER}/router/health`) },
  { id: "prometheus",       name: "Prometheus Metrics",      fn: async () => {
    const m = await api(`${ROUTER}/metrics`);
    const required = ["gateway_cache_hits_total","gateway_cache_misses_total","gateway_cache_hit_ratio","gateway_queue_depth","gateway_adaptive_limit_current"];
    const missing = required.filter(r => !m.includes(r));
    if (missing.length) throw new Error(`Missing: ${missing.join(", ")}`);
    return { found: required.length };
  }},
  { id: "admin_stats",      name: "Admin Stats Endpoint",    fn: async () => {
    const d = await api(`${ROUTER}/admin/stats/runtime`, { admin: true });
    const required = ["cache_hits_total","cache_misses_total","cache_hit_ratio","errors_total"];
    const missing = required.filter(k => !(k in d));
    if (missing.length) throw new Error(`Missing keys: ${missing.join(", ")}`);
    return d;
  }},
  { id: "cache_hit",        name: "Cache Records Hits",      fn: async () => {
    const d = await api(`${ROUTER}/admin/stats/runtime`, { admin: true });
    return { cache_hits: d.cache_hits_total, semantic_hits: d.semantic_hits_total };
  }},
  { id: "http_headers",     name: "HTTP Cache Headers",      fn: async () => {
    const token = $("authToken").value.trim() || "NASK_zzonHU_UdNhK1ZG-0GFGzA";
    const fd = new FormData(); fd.append("session_id", "chk"); fd.append("query", `test headers ${Date.now()}`);
    const r = await fetch(`${KONG}/router`, { method: "POST", headers: { "Authorization": `Bearer ${token}` }, body: fd });
    const xc = r.headers.get("x-cache");
    const xl = r.headers.get("x-agent-latency");
    if (!xc && !xl) throw new Error("No X-Cache or X-Agent-Latency headers. Make sure token is valid.");
    return { "X-Cache": xc, "X-Agent-Latency": xl, "X-Cache-Age": r.headers.get("x-cache-age") };
  }},
  { id: "cache_config",     name: "Cache Config Readable",   fn: async () => {
    const d = await api(`${ROUTER}/admin/stats/runtime`, { admin: true });
    const c = d.cache;
    if (!c || typeof c.ttl_seconds === "undefined") throw new Error("No cache config in stats");
    return c;
  }},
  { id: "rate_limits",      name: "Rate Limiter Active",     fn: async () => {
    const d = await api(`${ROUTER}/admin/stats/runtime`, { admin: true });
    return { agents: Object.keys(d.rate_limits || {}).length, rate_limits: d.rate_limits };
  }},
];

async function runChecks() {
  const btn = $("runChecks");
  const grid = $("checksGrid");
  btn.disabled = true;
  btn.textContent = "Running…";
  grid.innerHTML = "";

  for (const check of CHECKS) {
    const card = document.createElement("div");
    card.className = "check-card running";
    card.innerHTML = `<span class="check-icon">🔄</span><div class="check-body"><span class="check-name">${check.name}</span><span class="check-detail">Running…</span></div>`;
    grid.appendChild(card);
  }

  const cards = grid.querySelectorAll(".check-card");
  for (let i = 0; i < CHECKS.length; i++) {
    try {
      const result = await CHECKS[i].fn();
      cards[i].className = "check-card pass";
      cards[i].querySelector(".check-icon").textContent = "✅";
      const detail = typeof result === "object" ? JSON.stringify(result).slice(0, 60) : "OK";
      cards[i].querySelector(".check-detail").textContent = detail;
    } catch (e) {
      cards[i].className = "check-card fail";
      cards[i].querySelector(".check-icon").textContent = "❌";
      cards[i].querySelector(".check-detail").textContent = e.message;
    }
  }

  btn.disabled = false;
  btn.textContent = "Run All Checks";
}

$("runBenchmark").addEventListener("click", runBenchmark);
$("runChecks").addEventListener("click", runChecks);

$("flushCache").addEventListener("click", async () => {
  if (!confirm("Flush entire cache?")) return;
  try {
    await api(`${ROUTER}/admin/cache/clear`, { method: "POST", admin: true });
    await refresh();
  } catch (e) { alert("Flush failed: " + e.message); }
});

$("toggleMetrics").addEventListener("click", () => {
  const pre = $("metricsRaw");
  pre.classList.toggle("collapsed");
  const chev = $("toggleMetrics").querySelector(".chevron");
  chev.textContent = pre.classList.contains("collapsed") ? "▼" : "▲";
});

// --- Traffic Generator & Logs ---

function appendLog(reqData, resStatus, resHeaders, resBody, timeMs) {
  const logs = $("apiLogs");
  const isCacheHit = resHeaders["x-cache"] && resHeaders["x-cache"].includes("HIT");
  const isError = resStatus >= 400;
  const isRateLimit = resStatus === 429;
  
  // Remove placeholder if present
  const placeholder = logs.querySelector(".bench-placeholder");
  if (placeholder) placeholder.remove();

  let entryClass = "log-entry";
  if (isCacheHit) entryClass += " cache-hit";
  else if (isRateLimit) entryClass += " rate-limit";
  else if (isError) entryClass += " error";

  const statusClass = isError ? "fail" : "pass";
  const cacheHeader = resHeaders["x-cache"] || "MISS";
  const latHeader = resHeaders["x-agent-latency"] || "N/A";

  const entry = document.createElement("div");
  entry.className = entryClass;
  entry.innerHTML = `
    <div class="log-meta">
      <span><span class="log-status ${statusClass}">${resStatus}</span> • ${timeMs}ms</span>
      <span>X-Cache: ${cacheHeader} | Latency: ${latHeader}ms</span>
    </div>
    <div class="log-req">▶ POST /router | route=${reqData.route} | query="${reqData.query}"</div>
    <div class="log-res">${resBody}</div>
  `;
  
  logs.prepend(entry);
}

async function sendRouterRequest(query, route) {
  const t0 = performance.now();
  const fd = new FormData();
  fd.append("session_id", "demo-session");
  fd.append("query", query);
  if (route) fd.append("route", route);

  const token = $("authToken").value.trim() || "NASK_zzonHU_UdNhK1ZG-0GFGzA";

  try {
    const res = await fetch(`${KONG}/router`, {
      method: "POST",
      headers: { "Authorization": `Bearer ${token}` },
      body: fd
    });
    const ms = Math.round(performance.now() - t0);
    const headers = {
      "x-cache": res.headers.get("x-cache"),
      "x-agent-latency": res.headers.get("x-agent-latency")
    };
    const text = await res.text();
    appendLog({ query, route }, res.status, headers, text.trim().substring(0, 300) + (text.length > 300 ? "..." : ""), ms);
  } catch (e) {
    const ms = Math.round(performance.now() - t0);
    appendLog({ query, route }, 0, {}, `Network Error: ${e.message}`, ms);
  }
  
  // Force a quick refresh of stats
  refresh();
}

// Token persistence
const savedToken = localStorage.getItem("nasiko_dash_token");
if (savedToken) $("authToken").value = savedToken;
$("authToken").addEventListener("change", (e) => {
  localStorage.setItem("nasiko_dash_token", e.target.value.trim());
});

$("autoLoginBtn").addEventListener("click", async () => {
  const btn = $("autoLoginBtn");
  btn.textContent = "Logging in...";
  btn.disabled = true;
  try {
    const res = await fetch("/api/auto-login", { method: "POST" });
    if (!res.ok) throw new Error("Auto-login failed.");
    const data = await res.json();
    const token = data.token || data.access_token || (data.data && data.data.token);
    if (!token) throw new Error("Login succeeded but no token found in response.");
    $("authToken").value = token;
    localStorage.setItem("nasiko_dash_token", token);
    btn.textContent = "✅ Success";
  } catch(e) {
    alert(e.message);
    btn.textContent = "❌ Failed";
  }
  setTimeout(() => { btn.textContent = "Auto-Login (Admin)"; btn.disabled = false; }, 2000);
});

$("sendSingle").addEventListener("click", () => {
  const q = $("customQuery").value || "translate hello world to French";
  const r = $("customRoute").value || "a2a-translator";
  sendRouterRequest(q, r);
});

$("sendSpam").addEventListener("click", () => {
  const q = $("customQuery").value || "translate spam test";
  const r = $("customRoute").value || "a2a-translator";
  // Fire 5 requests concurrently to trigger limits/queues
  for (let i = 0; i < 5; i++) {
    setTimeout(() => sendRouterRequest(q + " " + i, r), i * 50);
  }
});

$("clearLogs").addEventListener("click", () => {
  $("apiLogs").innerHTML = `<div class="bench-placeholder">Send a request to see the exact payload, cache status, and rate limit info here.</div>`;
});

refresh();
pollTimer = setInterval(refresh, POLL_MS);
