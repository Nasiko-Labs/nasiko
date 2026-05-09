DASHBOARD_HTML = """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Request Manager · Nasiko</title>
<style>
*,*::before,*::after{box-sizing:border-box;margin:0;padding:0}
:root{
  --bg:#09090B;--surface:#111113;--surface2:#18181B;--border:#27272A;--border2:#1C1C1F;
  --text:#FAFAFA;--muted:#A1A1AA;--dim:#52525B;
  --purple:#8B5CF6;--purple-bg:#2D1B69;
  --green:#22C55E;--green-bg:#052E16;
  --yellow:#EAB308;--yellow-bg:#1C1506;
  --red:#EF4444;--red-bg:#1F0A0A;
  --blue:#3B82F6;--blue-bg:#0D1F4A;
  --r:8px;
}
html{font-size:14px}
body{font-family:'Inter',system-ui,-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:var(--bg);color:var(--text);min-height:100vh;line-height:1.5;-webkit-font-smoothing:antialiased}
.layout{display:flex;min-height:100vh}

/* sidebar */
.sidebar{width:220px;background:var(--surface);border-right:1px solid var(--border);flex-shrink:0;display:flex;flex-direction:column}
.logo{padding:18px 16px;border-bottom:1px solid var(--border);display:flex;align-items:center;gap:10px}
.logo-icon{width:28px;height:28px;background:var(--purple);border-radius:7px;display:flex;align-items:center;justify-content:center;font-size:13px;font-weight:700;color:#fff;flex-shrink:0}
.logo-name{font-size:14px;font-weight:600}
.logo-sub{font-size:10px;color:var(--muted);margin-top:1px}
.nav-section{padding:16px 12px 6px}
.nav-label{font-size:10px;font-weight:600;color:var(--dim);text-transform:uppercase;letter-spacing:1px;padding:0 6px;margin-bottom:4px}
.nav-item{display:flex;align-items:center;gap:9px;padding:7px 10px;border-radius:6px;cursor:pointer;font-size:13px;color:var(--muted);font-weight:450;margin-bottom:1px;user-select:none;transition:background .1s,color .1s}
.nav-item:hover{background:var(--surface2);color:var(--text)}
.nav-item.active{background:var(--purple-bg);color:var(--purple);font-weight:500}
.nav-icon{width:16px;text-align:center;flex-shrink:0}
.sidebar-footer{margin-top:auto;padding:14px 16px;border-top:1px solid var(--border);font-size:11px;color:var(--dim)}

/* topbar */
.main{flex:1;display:flex;flex-direction:column;min-width:0}
.topbar{background:var(--surface);border-bottom:1px solid var(--border);padding:0 24px;height:52px;display:flex;align-items:center;justify-content:space-between;flex-shrink:0}
.topbar-title{font-size:14px;font-weight:600}
.topbar-right{display:flex;align-items:center;gap:12px}
.badge{display:inline-flex;align-items:center;gap:5px;padding:3px 10px;border-radius:20px;font-size:11px;font-weight:500;border:1px solid var(--border);background:var(--surface2);color:var(--muted)}
.badge.ok{border-color:#14532D;background:var(--green-bg);color:var(--green)}
.badge.err{border-color:#450A0A;background:var(--red-bg);color:var(--red)}
.bdot{width:6px;height:6px;border-radius:50%;background:currentColor}
.ts{font-size:11px;color:var(--dim)}
.content{flex:1;overflow-y:auto;padding:24px}

/* views */
.view{display:none}.view.active{display:block}

/* cards */
.card{background:var(--surface);border:1px solid var(--border);border-radius:var(--r);padding:18px 20px}
.card+.card{margin-top:16px}
.card-title{font-size:11px;font-weight:600;color:var(--dim);text-transform:uppercase;letter-spacing:.8px;margin-bottom:14px}
.g4{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin-bottom:16px}
.g3{display:grid;grid-template-columns:repeat(3,1fr);gap:12px;margin-bottom:16px}
.g2{display:grid;grid-template-columns:1fr 1fr;gap:16px;margin-bottom:16px}
.kv{font-size:30px;font-weight:700;line-height:1.1;letter-spacing:-.5px}
.ks{font-size:11px;color:var(--dim);margin-top:5px}

/* colours */
.cp{color:var(--purple)}.cg{color:var(--green)}.cy{color:var(--yellow)}.cr{color:var(--red)}.cb{color:var(--blue)}

/* bar */
.bar-t{background:var(--surface2);border-radius:4px;height:6px;overflow:hidden;margin-top:8px}
.bar-f{height:100%;border-radius:4px;transition:width .5s}

/* table */
.tw{overflow-x:auto}
table{width:100%;border-collapse:collapse;font-size:12.5px}
th{text-align:left;font-size:10.5px;font-weight:600;color:var(--dim);text-transform:uppercase;letter-spacing:.6px;padding:7px 12px;border-bottom:1px solid var(--border);white-space:nowrap}
td{padding:9px 12px;border-bottom:1px solid var(--border2)}
tr:last-child td{border-bottom:none}
tbody tr:hover td{background:var(--surface2)}

/* tags */
.tag{display:inline-flex;align-items:center;padding:2px 8px;border-radius:4px;font-size:11px;font-weight:600}
.t-hit{background:var(--green-bg);color:var(--green)}
.t-miss{background:var(--blue-bg);color:var(--blue)}
.t-rej{background:var(--red-bg);color:var(--red)}
.t-custom{background:var(--purple-bg);color:var(--purple)}
.t-def{background:var(--surface2);color:var(--muted)}

/* buttons */
.btn{display:inline-flex;align-items:center;gap:6px;padding:7px 14px;border-radius:6px;font-size:12px;font-weight:500;cursor:pointer;border:1px solid var(--border);background:var(--surface2);color:var(--muted);font-family:inherit;transition:background .12s,color .12s;white-space:nowrap}
.btn:hover{background:var(--border);color:var(--text)}
.btn.danger{border-color:#450A0A;background:var(--red-bg);color:var(--red)}
.btn.danger:hover{background:#3b0909}
.btn.primary{border-color:var(--purple-bg);background:var(--purple-bg);color:var(--purple)}
.btn.primary:hover{background:#3b2080}
.btn.sm{padding:3px 10px;font-size:11px}

/* forms */
.field{display:flex;flex-direction:column;gap:4px}
.field label{font-size:12px;color:var(--muted)}
input,select{background:var(--surface2);border:1px solid var(--border);border-radius:6px;color:var(--text);font-family:inherit;font-size:12.5px;padding:7px 10px;outline:none;width:100%}
input:focus,select:focus{border-color:var(--purple)}
input::placeholder{color:var(--dim)}
.form-row{display:flex;align-items:flex-end;gap:10px;flex-wrap:wrap}
.form-row .field{flex:1;min-width:150px}

/* misc */
.mono{font-family:'SFMono-Regular',Consolas,monospace}
.trunc{max-width:260px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.lf{color:var(--green)}.lm{color:var(--yellow)}.ls{color:var(--red)}
.empty{color:var(--dim);font-size:12px;padding:24px 0;text-align:center}
.sh{display:flex;align-items:center;justify-content:space-between;margin-bottom:14px;flex-wrap:wrap;gap:8px}
.sh-t{font-size:13px;font-weight:600}
.sh-s{font-size:11px;color:var(--dim)}
.divider{height:1px;background:var(--border2);margin:16px 0}
.toast{position:fixed;bottom:24px;right:24px;background:var(--surface2);border:1px solid var(--border);border-radius:8px;padding:10px 18px;font-size:12.5px;color:var(--text);box-shadow:0 4px 20px #00000060;z-index:9999;transition:opacity .3s}
::-webkit-scrollbar{width:6px;height:6px}::-webkit-scrollbar-track{background:transparent}::-webkit-scrollbar-thumb{background:var(--border);border-radius:3px}
</style>
</head>
<body>
<div class="layout">

<aside class="sidebar">
  <div class="logo">
    <div class="logo-icon">N</div>
    <div><div class="logo-name">Nasiko</div><div class="logo-sub">Agent Platform</div></div>
  </div>
  <div class="nav-section">
    <div class="nav-label">Observability</div>
    <div class="nav-item active" data-view="overview"><span class="nav-icon">⬡</span>Overview</div>
    <div class="nav-item" id="nav-phoenix"><span class="nav-icon">◈</span>Phoenix Traces</div>
  </div>
  <div class="nav-section">
    <div class="nav-label">Management</div>
    <div class="nav-item" data-view="cache"><span class="nav-icon">◫</span>Cache</div>
    <div class="nav-item" data-view="ratelimits"><span class="nav-icon">◎</span>Rate Limits</div>
    <div class="nav-item" data-view="requests"><span class="nav-icon">◌</span>Requests</div>
  </div>
  <div class="sidebar-footer" id="footer-ts">Auto-refreshes every 5s</div>
</aside>

<div class="main">
  <div class="topbar">
    <span class="topbar-title" id="page-title">Overview</span>
    <div class="topbar-right">
      <span class="ts" id="refresh-ts"></span>
      <span class="badge" id="hbadge"><span class="bdot"></span><span id="htext">connecting</span></span>
    </div>
  </div>

  <div class="content">

    <!-- OVERVIEW -->
    <div class="view active" id="view-overview">
      <div class="g4">
        <div class="card"><div class="card-title">Total Requests</div><div class="kv cb" id="s-total">—</div></div>
        <div class="card"><div class="card-title">Cache Hits</div><div class="kv cg" id="s-hits">—</div><div class="ks" id="s-hr">—</div></div>
        <div class="card"><div class="card-title">Cache Misses</div><div class="kv cy" id="s-miss">—</div></div>
        <div class="card"><div class="card-title">Errors</div><div class="kv cr" id="s-err">—</div></div>
      </div>
      <div class="g3">
        <div class="card"><div class="card-title">Rate Limited</div><div class="kv cy" id="s-lim">—</div><div class="ks">held &amp; queued</div></div>
        <div class="card"><div class="card-title">Queued &amp; Served</div><div class="kv cp" id="s-q">—</div></div>
        <div class="card"><div class="card-title">Rejected</div><div class="kv cr" id="s-rej">—</div><div class="ks">queue full or timeout</div></div>
      </div>
      <div class="g2">
        <div class="card">
          <div class="sh"><span class="sh-t">Cache per agent</span></div>
          <div class="tw" id="ov-cache"><div class="empty">No data yet</div></div>
        </div>
        <div class="card">
          <div class="sh"><span class="sh-t">Rate limit events</span></div>
          <div id="ov-rl"><div class="empty">No traffic yet</div></div>
        </div>
      </div>
      <div class="card">
        <div class="sh"><span class="sh-t">Router decision cache</span><span class="sh-s">Caches which agent to route each query to — skips LLM selection on repeat queries</span></div>
        <div class="g4" style="margin-bottom:0" id="rc-stats">
          <div><div class="card-title">Hits</div><div class="kv cg" id="rc-hits">—</div></div>
          <div><div class="card-title">Misses</div><div class="kv cy" id="rc-miss">—</div></div>
          <div><div class="card-title">Hit Rate</div><div class="kv cp" id="rc-hr">—</div></div>
          <div><div class="card-title">Cached Decisions</div><div class="kv cb" id="rc-stored">—</div><div class="ks" id="rc-ttl"></div></div>
        </div>
      </div>
    </div>

    <!-- CACHE -->
    <div class="view" id="view-cache">
      <div class="card">
        <div class="sh">
          <span class="sh-t">Cache stats</span>
          <button class="btn danger" id="btn-clear-all">Clear all cache</button>
        </div>
        <div class="tw" id="cache-tbl"><div class="empty">No data yet</div></div>
      </div>
      <div class="card">
        <div class="sh"><span class="sh-t">Clear agent cache</span></div>
        <div class="form-row">
          <div class="field" style="max-width:300px">
            <label>Agent container name</label>
            <input id="cache-agent-in" placeholder="e.g. agent-a2a-translator">
          </div>
          <button class="btn danger" id="btn-clear-agent">Clear</button>
        </div>
      </div>
    </div>

    <!-- RATE LIMITS -->
    <div class="view" id="view-ratelimits">
      <div class="card">
        <div class="sh"><span class="sh-t">Set rate limit</span></div>
        <div class="form-row" style="margin-bottom:20px">
          <div class="field">
            <label>Agent name</label>
            <input id="rl-agent" placeholder="e.g. agent-a2a-translator">
          </div>
          <div class="field" style="max-width:150px">
            <label>Requests / minute</label>
            <input id="rl-rpm" type="number" min="1" placeholder="60">
          </div>
          <button class="btn primary" id="btn-set-rl">Apply limit</button>
        </div>
        <div class="divider"></div>
        <div class="sh" style="margin-top:4px"><span class="sh-t">Reset to default</span></div>
        <div class="form-row">
          <div class="field" style="max-width:300px">
            <label>Agent name</label>
            <input id="rl-reset" placeholder="e.g. agent-a2a-translator">
          </div>
          <button class="btn danger" id="btn-reset-rl">Reset</button>
        </div>
      </div>
      <div class="card">
        <div class="sh"><span class="sh-t">Current limits</span></div>
        <div class="tw" id="rl-tbl"><div class="empty">No data yet</div></div>
      </div>
    </div>

    <!-- REQUESTS -->
    <div class="view" id="view-requests">
      <div class="card">
        <div class="sh">
          <span class="sh-t">Recent requests</span>
          <div style="display:flex;align-items:center;gap:10px">
            <label style="font-size:12px;color:var(--muted);white-space:nowrap">Filter agent</label>
            <select id="req-filter" style="width:220px;padding:5px 10px"></select>
            <span class="sh-s" id="req-count"></span>
          </div>
        </div>
        <div class="tw" id="req-tbl"><div class="empty">No requests yet</div></div>
      </div>
    </div>

  </div>
</div>
</div>

<script>
var _allReqs = [];
var _limitsData = {};

/* ── Navigation ── */
var TITLES = {overview:'Overview', cache:'Cache Management', ratelimits:'Rate Limits', requests:'Recent Requests'};
document.querySelectorAll('.nav-item[data-view]').forEach(function(el) {
  el.addEventListener('click', function() {
    var view = this.dataset.view;
    document.querySelectorAll('.view').forEach(function(v){v.classList.remove('active')});
    document.getElementById('view-'+view).classList.add('active');
    document.querySelectorAll('.nav-item').forEach(function(n){n.classList.remove('active')});
    this.classList.add('active');
    document.getElementById('page-title').textContent = TITLES[view] || view;
  });
});
document.getElementById('nav-phoenix').addEventListener('click', function() {
  window.open('http://localhost:6006','_blank');
});

/* ── Toast ── */
function toast(msg, col) {
  var t = document.createElement('div');
  t.className = 'toast';
  if (col) t.style.borderColor = col;
  t.textContent = msg;
  document.body.appendChild(t);
  setTimeout(function(){ t.style.opacity='0'; setTimeout(function(){t.remove()},300); }, 2500);
}

/* ── Helpers ── */
function fmt(n) { return (n == null) ? '0' : Number(n).toLocaleString(); }
function age(ts) {
  var d = Math.floor(Date.now()/1000 - ts);
  if (d < 5)    return 'just now';
  if (d < 60)   return d+'s ago';
  if (d < 3600) return Math.floor(d/60)+'m ago';
  return Math.floor(d/3600)+'h ago';
}
function latCls(ms) { return ms < 10 ? 'lf' : ms < 500 ? 'lm' : 'ls'; }
function tagHtml(c) {
  if (c === 'HIT')  return '<span class="tag t-hit">HIT</span>';
  if (c === 'MISS') return '<span class="tag t-miss">MISS</span>';
  return '<span class="tag t-rej">'+c+'</span>';
}
function esc(s){ return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }

/* ── Cache actions ── */
document.getElementById('btn-clear-all').addEventListener('click', function() {
  if (!confirm('Clear ALL cached responses across all agents?')) return;
  fetch('/manage/cache', {method:'DELETE'}).then(function(r){return r.json()}).then(function(d){
    toast('Cleared '+( d.cleared_keys||0)+' keys', 'var(--green)');
    fetchAll();
  }).catch(function(e){ toast('Error: '+e.message, 'var(--red)'); });
});

document.getElementById('btn-clear-agent').addEventListener('click', function() {
  var agent = document.getElementById('cache-agent-in').value.trim();
  if (!agent) { toast('Enter an agent name', 'var(--yellow)'); return; }
  fetch('/manage/cache/'+encodeURIComponent(agent), {method:'DELETE'}).then(function(r){return r.json()}).then(function(d){
    toast('Cleared '+(d.cleared_keys||0)+' keys for '+agent, 'var(--green)');
    document.getElementById('cache-agent-in').value = '';
    fetchAll();
  }).catch(function(e){ toast('Error: '+e.message, 'var(--red)'); });
});

/* ── Rate limit actions ── */
document.getElementById('btn-set-rl').addEventListener('click', function() {
  var agent = document.getElementById('rl-agent').value.trim();
  var rpm   = parseInt(document.getElementById('rl-rpm').value);
  if (!agent) { toast('Enter an agent name', 'var(--yellow)'); return; }
  if (!rpm || rpm < 1) { toast('Enter a valid requests/minute value', 'var(--yellow)'); return; }
  fetch('/manage/rate-limits/'+encodeURIComponent(agent), {
    method:'POST', headers:{'Content-Type':'application/json'},
    body: JSON.stringify({requests_per_minute: rpm})
  }).then(function(r){
    if (!r.ok) throw new Error('Server error '+r.status);
    toast('Set '+agent+' to '+rpm+' req/min', 'var(--green)');
    document.getElementById('rl-agent').value = '';
    document.getElementById('rl-rpm').value   = '';
    fetchAll();
  }).catch(function(e){ toast('Error: '+e.message, 'var(--red)'); });
});

document.getElementById('btn-reset-rl').addEventListener('click', function() {
  var agent = document.getElementById('rl-reset').value.trim();
  if (!agent) { toast('Enter an agent name', 'var(--yellow)'); return; }
  fetch('/manage/rate-limits/'+encodeURIComponent(agent), {method:'DELETE'}).then(function(){
    toast('Reset '+agent+' to default', 'var(--green)');
    document.getElementById('rl-reset').value = '';
    fetchAll();
  }).catch(function(e){ toast('Error: '+e.message, 'var(--red)'); });
});

/* Table-level delegated edit/reset for rate limits */
document.getElementById('rl-tbl').addEventListener('click', function(e) {
  var btn   = e.target.closest('button');
  if (!btn) return;
  var agent = btn.dataset.agent;
  if (!agent) return;
  if (btn.dataset.action === 'edit') {
    document.getElementById('rl-agent').value = agent;
    document.getElementById('rl-rpm').focus();
  } else if (btn.dataset.action === 'reset') {
    fetch('/manage/rate-limits/'+encodeURIComponent(agent), {method:'DELETE'}).then(function(){
      toast('Reset '+agent+' to default', 'var(--green)');
      fetchAll();
    });
  }
});

/* Table-level delegated clear for cache */
document.getElementById('cache-tbl').addEventListener('click', function(e) {
  var btn = e.target.closest('button');
  if (!btn || !btn.dataset.agent) return;
  var agent = btn.dataset.agent;
  fetch('/manage/cache/'+encodeURIComponent(agent), {method:'DELETE'}).then(function(r){return r.json()}).then(function(d){
    toast('Cleared '+(d.cleared_keys||0)+' keys for '+agent, 'var(--green)');
    fetchAll();
  });
});

/* ── Requests filter ── */
document.getElementById('req-filter').addEventListener('change', renderRequests);
function renderRequests() {
  var filter = document.getElementById('req-filter').value;
  var rows = filter ? _allReqs.filter(function(r){return r.agent === filter}) : _allReqs;
  document.getElementById('req-count').textContent = rows.length + ' requests';
  if (!rows.length) { document.getElementById('req-tbl').innerHTML = '<div class="empty">No requests</div>'; return; }
  var h = '<table><thead><tr><th>Time</th><th>Agent</th><th>Query</th><th>Cache</th><th>Latency</th><th>Flags</th></tr></thead><tbody>';
  rows.forEach(function(r) {
    var flags = [];
    if (r.queued) flags.push('<span style="color:var(--purple);font-size:11px">queued</span>');
    if (r.error)  flags.push('<span style="color:var(--red);font-size:11px">error</span>');
    h += '<tr><td class="mono" style="font-size:11px;color:var(--dim);white-space:nowrap">'+age(r.ts)+'</td>'+
         '<td><span class="mono cp" style="font-size:11.5px">'+esc(r.agent)+'</span></td>'+
         '<td><div class="trunc" title="'+esc(r.query||'')+'">'+esc(r.query||'—')+'</div></td>'+
         '<td>'+tagHtml(r.cache)+'</td>'+
         '<td><span class="mono '+latCls(r.latency_ms)+'">'+r.latency_ms+'ms</span></td>'+
         '<td>'+(flags.join(' ') || '<span style="color:var(--dim)">—</span>')+'</td></tr>';
  });
  h += '</tbody></table>';
  document.getElementById('req-tbl').innerHTML = h;
}

/* ── Main fetch loop ── */
function fetchAll() {
  Promise.all([
    fetch('/manage/stats').then(function(r){return r.json()}).catch(function(){return {}}),
    fetch('/manage/cache/stats').then(function(r){return r.json()}).catch(function(){return {}}),
    fetch('/manage/rate-limits').then(function(r){return r.json()}).catch(function(){return {}}),
    fetch('/manage/requests?limit=50').then(function(r){return r.json()}).catch(function(){return []}),
    fetch('/manage/health').then(function(r){return r.json()}).catch(function(){return {}}),
    fetch('http://127.0.0.1:8081/route-cache/stats').then(function(r){return r.json()}).catch(function(){return null})
  ]).then(function(results) {
    var statsR = results[0], cacheR = results[1], limitsR = results[2], reqsR = results[3], healthR = results[4], rcR = results[5];

    /* router decision cache */
    if (rcR && rcR.enabled !== false) {
      document.getElementById('rc-hits').textContent   = fmt(rcR.hits);
      document.getElementById('rc-miss').textContent   = fmt(rcR.misses);
      document.getElementById('rc-hr').textContent     = rcR.total_lookups ? (rcR.hit_rate*100).toFixed(1)+'%' : '—';
      document.getElementById('rc-stored').textContent = fmt(rcR.cached_decisions);
      document.getElementById('rc-ttl').textContent    = 'TTL '+rcR.ttl_seconds+'s';
    } else {
      ['rc-hits','rc-miss','rc-hr','rc-stored'].forEach(function(id){document.getElementById(id).textContent='—'});
      document.getElementById('rc-ttl').textContent = rcR ? 'disabled' : 'router unreachable';
    }

    /* health */
    var ok = healthR.status === 'healthy';
    document.getElementById('hbadge').className = 'badge '+(ok?'ok':'err');
    document.getElementById('htext').textContent = ok ? 'healthy' : 'degraded';
    var ts = new Date().toLocaleTimeString();
    document.getElementById('refresh-ts').textContent = 'Updated '+ts;
    document.getElementById('footer-ts').textContent  = 'Last updated '+ts;

    /* KPIs */
    var g = statsR.global || {};
    document.getElementById('s-total').textContent = fmt(g.total_requests);
    document.getElementById('s-hits').textContent  = fmt(g.cache_hits);
    document.getElementById('s-miss').textContent  = fmt(g.cache_misses);
    document.getElementById('s-err').textContent   = fmt(g.errors);
    document.getElementById('s-lim').textContent   = fmt(g.rate_limited);
    document.getElementById('s-q').textContent     = fmt(g.queued);
    document.getElementById('s-rej').textContent   = fmt((g.rejected_queue_full||0)+(g.rejected_timeout||0));
    var tot = (g.cache_hits||0)+(g.cache_misses||0);
    document.getElementById('s-hr').textContent = tot ? ((g.cache_hits||0)/tot*100).toFixed(1)+'% hit rate' : '—';

    /* cache tables */
    var ce = Object.entries(cacheR);
    function buildCacheTable(withActions) {
      if (!ce.length) return '<div class="empty">No cache data yet</div>';
      var h = '<table><thead><tr><th>Agent</th><th>Hits</th><th>Misses</th><th>Hit rate</th><th style="width:100px"></th>';
      if (withActions) h += '<th>Action</th>';
      h += '</tr></thead><tbody>';
      ce.forEach(function(entry) {
        var agent = entry[0], d = entry[1];
        var pct = Math.round((d.hit_rate||0)*100);
        var col = pct>60?'var(--green)':pct>30?'var(--yellow)':'var(--red)';
        h += '<tr><td><span class="mono cp" style="font-size:11.5px">'+esc(agent)+'</span></td>'+
             '<td class="cg">'+fmt(d.hits)+'</td><td class="cy">'+fmt(d.misses)+'</td><td>'+pct+'%</td>'+
             '<td><div class="bar-t"><div class="bar-f" style="width:'+pct+'%;background:'+col+'"></div></div></td>';
        if (withActions) h += '<td><button class="btn danger sm" data-agent="'+esc(agent)+'">Clear</button></td>';
        h += '</tr>';
      });
      return h+'</tbody></table>';
    }
    document.getElementById('ov-cache').innerHTML    = buildCacheTable(false);
    document.getElementById('cache-tbl').innerHTML   = buildCacheTable(true);

    /* ov rate-limit summary */
    var def = limitsR.default || {};
    var overrides = limitsR.per_agent_overrides || {};
    var rlEv = (statsR.rate_limit_events) || {};
    var agents = Object.keys(Object.assign({}, overrides, rlEv));
    var ovHtml = '<div style="display:flex;justify-content:space-between;padding:8px 0;border-bottom:1px solid var(--border2)"><span style="color:var(--muted);font-size:12px">Default</span><span class="mono" style="font-size:11px;color:var(--muted)">'+(def.requests_per_minute||60)+' req/min</span></div>';
    agents.forEach(function(a) {
      var ev = rlEv[a]||{};
      var rpm = (overrides[a]&&overrides[a].requests_per_minute) || def.requests_per_minute || 60;
      ovHtml += '<div style="padding:10px 0;border-bottom:1px solid var(--border2)">'+
        '<div style="display:flex;justify-content:space-between;align-items:center">'+
        '<span class="mono cp" style="font-size:12px">'+esc(a)+'</span>'+
        '<span class="mono" style="font-size:11px;color:var(--muted)">'+rpm+' req/min'+(overrides[a]?' <span style="color:var(--purple);font-size:10px">custom</span>':'')+'</span></div>'+
        '<div style="font-size:11px;color:var(--dim);margin-top:3px">✓ '+fmt(ev.allowed)+' allowed &nbsp;·&nbsp; ⏸ '+fmt(ev.limited)+' limited &nbsp;·&nbsp; ⏳ '+fmt(ev.queued)+' queued</div>'+
        '</div>';
    });
    if (!agents.length) ovHtml += '<div class="empty">No agent traffic yet</div>';
    document.getElementById('ov-rl').innerHTML = ovHtml;

    /* rate limits table */
    var rlHtml = '<table><thead><tr><th>Agent</th><th>RPM</th><th>Type</th><th>Allowed</th><th>Limited</th><th>Queued</th><th>Actions</th></tr></thead><tbody>';
    rlHtml += '<tr><td style="color:var(--dim)">Default</td><td>'+(def.requests_per_minute||60)+'</td><td><span class="tag t-def">global</span></td><td colspan="4" style="color:var(--dim)">—</td></tr>';
    agents.forEach(function(a) {
      var ev = rlEv[a]||{};
      var isCustom = !!overrides[a];
      var rpm = (isCustom && overrides[a].requests_per_minute) || def.requests_per_minute || 60;
      rlHtml += '<tr>'+
        '<td><span class="mono cp" style="font-size:11.5px">'+esc(a)+'</span></td>'+
        '<td>'+rpm+'</td>'+
        '<td>'+(isCustom?'<span class="tag t-custom">custom</span>':'<span class="tag t-def">default</span>')+'</td>'+
        '<td class="cg">'+fmt(ev.allowed)+'</td>'+
        '<td class="cy">'+fmt(ev.limited)+'</td>'+
        '<td class="cp">'+fmt(ev.queued)+'</td>'+
        '<td style="display:flex;gap:6px;flex-wrap:wrap">'+
          '<button class="btn sm" data-agent="'+esc(a)+'" data-action="edit">Edit</button>'+
          (isCustom?'<button class="btn danger sm" data-agent="'+esc(a)+'" data-action="reset">Reset</button>':'') +
        '</td></tr>';
    });
    if (!agents.length) rlHtml += '<tr><td colspan="7" class="empty" style="text-align:center">No agent rate limit data yet</td></tr>';
    rlHtml += '</tbody></table>';
    document.getElementById('rl-tbl').innerHTML = rlHtml;

    /* requests */
    _allReqs = Array.isArray(reqsR) ? reqsR : [];
    var names = [];
    _allReqs.forEach(function(r){ if (names.indexOf(r.agent)<0) names.push(r.agent); });
    var sel = document.getElementById('req-filter');
    var prev = sel.value;
    sel.innerHTML = '<option value="">All agents</option>';
    names.forEach(function(n){
      var opt = document.createElement('option');
      opt.value = n; opt.textContent = n;
      if (n === prev) opt.selected = true;
      sel.appendChild(opt);
    });
    renderRequests();
  });
}

fetchAll();
setInterval(fetchAll, 5000);
</script>
</body>
</html>"""
