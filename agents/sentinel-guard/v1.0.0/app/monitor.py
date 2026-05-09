"""
Real-time monitoring dashboard for Sentinel Guard.
Serves an embedded HTML dashboard with SSE live updates.
"""

DASHBOARD_HTML = """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Sentinel Guard — Monitoring Dashboard</title>
<link href="https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700&display=swap" rel="stylesheet">
<style>
*{margin:0;padding:0;box-sizing:border-box}
:root{
  --bg:#0b0e17;--card:#12162a;--card-hover:#181e38;
  --border:#1e2540;--text:#e4e8f7;--muted:#7a829e;
  --accent:#6c63ff;--accent2:#00d4aa;--accent3:#ff6b6b;
  --success:#00d4aa;--warning:#ffb74d;--danger:#ff5252;
}
body{font-family:'Inter',sans-serif;background:var(--bg);color:var(--text);min-height:100vh}
.header{
  background:linear-gradient(135deg,#0f1326 0%,#1a1f3a 100%);
  border-bottom:1px solid var(--border);padding:20px 32px;
  display:flex;align-items:center;gap:16px;
}
.header h1{font-size:22px;font-weight:700;background:linear-gradient(90deg,var(--accent),var(--accent2));
  -webkit-background-clip:text;-webkit-text-fill-color:transparent}
.status-dot{width:10px;height:10px;border-radius:50%;background:var(--success);
  box-shadow:0 0 8px var(--success);animation:pulse 2s infinite}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.5}}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));
  gap:20px;padding:24px 32px}
.card{
  background:var(--card);border:1px solid var(--border);border-radius:16px;
  padding:24px;transition:all .3s;position:relative;overflow:hidden;
}
.card::before{content:'';position:absolute;top:0;left:0;right:0;height:3px;
  background:linear-gradient(90deg,var(--accent),var(--accent2));opacity:0;transition:opacity .3s}
.card:hover{background:var(--card-hover);transform:translateY(-2px);
  box-shadow:0 8px 24px rgba(0,0,0,.3)}
.card:hover::before{opacity:1}
.card-title{font-size:12px;text-transform:uppercase;letter-spacing:1.5px;color:var(--muted);margin-bottom:12px}
.card-value{font-size:36px;font-weight:700}
.card-sub{font-size:13px;color:var(--muted);margin-top:6px}
.accent-val{color:var(--accent)}
.success-val{color:var(--success)}
.warning-val{color:var(--warning)}
.danger-val{color:var(--danger)}
.wide{grid-column:1/-1}
.table-wrap{overflow-x:auto}
table{width:100%;border-collapse:collapse;font-size:13px}
th{text-align:left;padding:10px 12px;color:var(--muted);font-weight:500;
  border-bottom:1px solid var(--border);font-size:11px;text-transform:uppercase;letter-spacing:1px}
td{padding:10px 12px;border-bottom:1px solid rgba(255,255,255,.04)}
tr:hover td{background:rgba(108,99,255,.04)}
.badge{display:inline-block;padding:3px 10px;border-radius:20px;font-size:11px;font-weight:600}
.badge-hit{background:rgba(0,212,170,.15);color:var(--success)}
.badge-miss{background:rgba(255,82,82,.1);color:var(--danger)}
.badge-queued{background:rgba(255,183,77,.12);color:var(--warning)}
.badge-rate{background:rgba(108,99,255,.12);color:var(--accent)}
.controls{display:flex;gap:12px;flex-wrap:wrap;padding:0 32px 24px}
.btn{background:var(--card);border:1px solid var(--border);color:var(--text);
  padding:8px 18px;border-radius:10px;cursor:pointer;font-size:13px;
  font-family:inherit;transition:all .2s}
.btn:hover{background:var(--accent);border-color:var(--accent);color:#fff}
.btn-danger:hover{background:var(--danger);border-color:var(--danger)}
.progress-bar{height:6px;background:var(--border);border-radius:3px;margin-top:8px;overflow:hidden}
.progress-fill{height:100%;border-radius:3px;transition:width .5s ease}
@media(max-width:768px){.grid{grid-template-columns:1fr;padding:16px}.header{padding:16px}}
</style>
</head>
<body>
<div class="header">
  <div class="status-dot" id="live-dot"></div>
  <h1>Sentinel Guard</h1>
  <span style="color:var(--muted);font-size:13px;margin-left:auto" id="last-update">Connecting...</span>
</div>

<div class="grid" id="metrics-grid">
  <div class="card"><div class="card-title">Cache Hit Rate</div>
    <div class="card-value success-val" id="hit-rate">—</div>
    <div class="card-sub" id="hit-counts">Calculating...</div>
    <div class="progress-bar"><div class="progress-fill" id="hit-bar" style="width:0;background:var(--success)"></div></div>
  </div>
  <div class="card"><div class="card-title">Total Requests</div>
    <div class="card-value accent-val" id="total-req">0</div>
    <div class="card-sub" id="req-breakdown">—</div>
  </div>
  <div class="card"><div class="card-title">Avg Cache Hit Latency</div>
    <div class="card-value" id="avg-latency">—</div>
    <div class="card-sub">Target: &lt; 50ms</div>
  </div>
  <div class="card"><div class="card-title">Queue Depth</div>
    <div class="card-value warning-val" id="queue-depth">0</div>
    <div class="card-sub" id="queue-sub">No active queues</div>
  </div>
</div>

<div class="controls">
  <button class="btn" onclick="flushCache()">🗑️ Flush All Cache</button>
  <button class="btn" onclick="refreshStats()">🔄 Refresh</button>
</div>

<div class="grid">
  <div class="card wide">
    <div class="card-title">Per-Agent Statistics</div>
    <div class="table-wrap">
      <table><thead><tr>
        <th>Agent</th><th>Requests</th><th>Cache Hits</th><th>Semantic Hits</th>
        <th>Hit Rate</th><th>Rate Limit</th><th>Queue</th><th>Status</th>
      </tr></thead><tbody id="agent-table">
        <tr><td colspan="8" style="text-align:center;color:var(--muted)">Waiting for data...</td></tr>
      </tbody></table>
    </div>
  </div>

  <div class="card wide">
    <div class="card-title">Recent Decisions</div>
    <div class="table-wrap">
      <table><thead><tr>
        <th>Time</th><th>Agent</th><th>Query</th><th>Outcome</th><th>Latency</th><th>Similarity</th>
      </tr></thead><tbody id="decision-table">
        <tr><td colspan="6" style="text-align:center;color:var(--muted)">Waiting for data...</td></tr>
      </tbody></table>
    </div>
  </div>
</div>

<script>
const evtSource = new EventSource('/events');
evtSource.onmessage = function(e) {
  try { const d = JSON.parse(e.data); update(d); } catch(err) { console.error(err); }
};
evtSource.onerror = function() {
  document.getElementById('live-dot').style.background = 'var(--danger)';
  document.getElementById('last-update').textContent = 'Disconnected';
};

function update(d) {
  document.getElementById('last-update').textContent = 'Live — ' + new Date().toLocaleTimeString();
  document.getElementById('live-dot').style.background = 'var(--success)';

  const s = d.summary || {};
  const hitRate = s.cache_hit_rate_pct || 0;
  document.getElementById('hit-rate').textContent = hitRate.toFixed(1) + '%';
  document.getElementById('hit-bar').style.width = hitRate + '%';
  document.getElementById('hit-counts').textContent =
    `${s.total_cache_hits||0} hits / ${s.total_cache_misses||0} misses`;
  document.getElementById('total-req').textContent = s.total_requests || 0;
  document.getElementById('req-breakdown').textContent =
    `${s.total_forwarded||0} forwarded • ${s.total_queued||0} queued • ${s.total_rejected||0} rejected`;
  document.getElementById('avg-latency').textContent =
    (s.avg_cache_hit_latency_ms||0).toFixed(1) + 'ms';
  document.getElementById('queue-depth').textContent = s.total_queue_depth || 0;
  document.getElementById('queue-sub').textContent =
    (s.total_queue_depth > 0) ? 'Active queues' : 'No active queues';

  // Agent table
  const agents = d.per_agent || {};
  const tbody = document.getElementById('agent-table');
  if (Object.keys(agents).length === 0) {
    tbody.innerHTML = '<tr><td colspan="8" style="text-align:center;color:var(--muted)">No agents yet</td></tr>';
  } else {
    tbody.innerHTML = Object.entries(agents).map(([name, a]) => {
      const hr = a.total > 0 ? (a.hits / a.total * 100).toFixed(1) : '0.0';
      return `<tr>
        <td><strong>${name}</strong></td>
        <td>${a.total||0}</td><td>${a.hits||0}</td><td>${a.semantic_hits||0}</td>
        <td><span class="badge ${parseFloat(hr)>50?'badge-hit':'badge-miss'}">${hr}%</span></td>
        <td>${a.rate_limit||60} RPM</td><td>${a.queue_depth||0}</td>
        <td><span class="badge badge-hit">Active</span></td>
      </tr>`;
    }).join('');
  }

  // Decision log
  const decisions = d.recent_decisions || [];
  const dtbody = document.getElementById('decision-table');
  if (decisions.length === 0) {
    dtbody.innerHTML = '<tr><td colspan="6" style="text-align:center;color:var(--muted)">No decisions yet</td></tr>';
  } else {
    dtbody.innerHTML = decisions.slice(0, 25).map(dec => {
      const t = new Date(dec.timestamp * 1000).toLocaleTimeString();
      const badge = dec.outcome.includes('hit') ? 'badge-hit' :
                    dec.outcome === 'queued' ? 'badge-queued' :
                    dec.outcome === 'rate_limited' ? 'badge-rate' : 'badge-miss';
      return `<tr>
        <td>${t}</td><td>${dec.agent}</td>
        <td title="${dec.query}">${(dec.query||'').substring(0,50)}${(dec.query||'').length>50?'...':''}</td>
        <td><span class="badge ${badge}">${dec.outcome}</span></td>
        <td>${dec.latency_ms ? dec.latency_ms.toFixed(1)+'ms' : '—'}</td>
        <td>${dec.similarity ? dec.similarity.toFixed(3) : '—'}</td>
      </tr>`;
    }).join('');
  }
}

async function flushCache() {
  if (!confirm('Flush all cached responses?')) return;
  const r = await fetch('/cache/flush', {method:'POST'});
  const d = await r.json();
  alert('Flushed: ' + JSON.stringify(d.flushed));
}

async function refreshStats() {
  const r = await fetch('/stats');
  const d = await r.json();
  update(d);
}
</script>
</body>
</html>"""
