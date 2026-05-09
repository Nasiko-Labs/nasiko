'use strict';

const POLL_MS = 2000;

// Latest recommendations keyed by agentId — refreshed every poll cycle
let latestRecs = {};

// ── Helpers ───────────────────────────────────────────────────────────────────

function fmt(v) {
  if (v === null || v === undefined) return '—';
  if (typeof v === 'number') return v.toLocaleString();
  return String(v);
}

function fmtMs(v) {
  if (v === null || v === undefined) return '—';
  return v + ' ms';
}

function hitRate(hits, misses) {
  const total = hits + misses;
  if (total === 0) return null;
  return (hits / total) * 100;
}

// CSS class based on cache hit-rate percentage
function rateClass(pct) {
  if (pct === null) return '';
  if (pct >= 70) return 'green';
  if (pct >= 40) return 'amber';
  return 'red';
}

// Health status of an agent based on error rate, drops, queue fill
function agentStatus(a) {
  const errorRate = a.total > 0 ? a.errorCount / a.total : 0;
  const q   = a.queue       || {};
  const rl  = a.rateLimiter || {};
  const cfg = rl.config     || {};
  const qFill = q.maxQueueLength > 0 ? (q.depth ?? 0) / q.maxQueueLength : 0;

  if (a.dropped > 0 || errorRate > 0.3) return { label: 'Degraded', cls: 'red'   };
  if (errorRate > 0.1 || qFill > 0.6)   return { label: 'Warning',  cls: 'amber' };
  if ((q.depth ?? 0) > 0)               return { label: 'Queuing',  cls: 'blue'  };
  return { label: 'Healthy', cls: 'green' };
}

// Fill color for the in-flight concurrency bar
function inflightColor(pct) {
  if (pct < 60) return '#059669';
  if (pct < 85) return '#d97706';
  return '#e11d48';
}

// Fill color for the token-bucket progress bar
function tokenColor(pct) {
  if (pct > 60) return '#059669'; // green-d
  if (pct > 25) return '#d97706'; // amber-d
  return '#e11d48';               // red-d
}

// Fill color for the queue-depth progress bar
function queueColor(pct) {
  if (pct < 40) return '#2563eb'; // blue-d
  if (pct < 75) return '#d97706'; // amber-d
  return '#e11d48';               // red-d
}

// ── KPI strip renderer ────────────────────────────────────────────────────────

function renderStats(cacheData, globalData) {
  const hits   = cacheData.hits   ?? 0;
  const misses = cacheData.misses ?? 0;
  const pct    = hitRate(hits, misses);

  document.getElementById('kpi-reqs').textContent    = fmt(globalData.totalRequests);
  document.getElementById('kpi-hits').textContent    = fmt(hits);
  document.getElementById('kpi-misses').textContent  = fmt(misses);
  document.getElementById('kpi-entries').textContent = fmt(cacheData.size);

  const rateEl    = document.getElementById('kpi-rate');
  const rateSubEl = document.getElementById('kpi-rate-sub');

  if (pct !== null) {
    rateEl.textContent    = pct.toFixed(1) + '%';
    rateEl.className      = 'kpi-value ' + rateClass(pct);
    rateSubEl.textContent = `${hits} hits out of ${hits + misses} requests`;
  } else {
    rateEl.textContent    = '—';
    rateEl.className      = 'kpi-value';
    rateSubEl.textContent = 'no requests yet';
  }

  // Workflow-scoped dedupe note (driven by global counter from /ops/agents/stats)
  const dedupeEl = document.getElementById('kpi-dedupe-note');
  if (dedupeEl) {
    const wdh = globalData.workflowDedupeHits ?? 0;
    dedupeEl.textContent = wdh > 0
      ? `${wdh} workflow dedupe hit${wdh !== 1 ? 's' : ''}`
      : '';
  }
}

// ── SLO row helper ────────────────────────────────────────────────────────────

function renderSloRow(a) {
  const status  = a.sloStatus ?? 'unknown';
  const hasSlo  = a.sloP95Ms !== undefined && a.sloP95Ms !== null;
  const target  = hasSlo
    ? `${a.sloP95Ms} ms${a.sloLabel ? ` &middot; ${a.sloLabel}` : ''}`
    : '';

  let valHtml;
  if (status === 'ok') {
    valHtml = `<span class="slo-val slo-ok">${target} &nbsp;&middot;&nbsp; ✓ OK</span>`;
  } else if (status === 'breaching') {
    valHtml = `<span class="slo-val slo-breaching">${target} &nbsp;&middot;&nbsp; ✗ Breaching</span>`;
  } else if (hasSlo) {
    // SLO configured but no p95 data yet
    valHtml = `<span class="slo-val slo-unknown">${target} &nbsp;&middot;&nbsp; No data</span>`;
  } else {
    valHtml = `<span class="slo-val slo-unknown">Not configured</span>`;
  }

  return `<div class="slo-row"><span class="slo-lbl">SLO</span>${valHtml}</div>`;
}

// ── Advisory renderer ─────────────────────────────────────────────────────────

function renderAdvisory(agentId) {
  const rec = latestRecs[agentId];

  if (!rec) {
    return `<div class="advisory">
      <span class="advisory-hd">Advisory</span>
      <span class="advisory-body advisory-dim">Awaiting first analysis…</span>
    </div>`;
  }

  const hasSuggestions = (
    rec.suggestedCapacity         !== undefined ||
    rec.suggestedRefillRatePerSec !== undefined ||
    rec.suggestedMaxQueueLength   !== undefined ||
    rec.suggestedMaxConcurrent    !== undefined
  );

  if (!hasSuggestions) {
    return `<div class="advisory">
      <span class="advisory-hd">Advisory</span>
      <span class="advisory-body advisory-ok">✓ No changes recommended</span>
    </div>`;
  }

  const parts = [];
  if (rec.suggestedCapacity         !== undefined) parts.push(`capacity → <b>${rec.suggestedCapacity}</b>`);
  if (rec.suggestedRefillRatePerSec !== undefined) parts.push(`refill → <b>${rec.suggestedRefillRatePerSec}/sec</b>`);
  if (rec.suggestedMaxQueueLength   !== undefined) parts.push(`queue → <b>${rec.suggestedMaxQueueLength}</b>`);
  if (rec.suggestedMaxConcurrent    !== undefined) parts.push(`concurrent → <b>${rec.suggestedMaxConcurrent}</b>`);

  return `<div class="advisory warn">
    <span class="advisory-hd">⚡ Advisory</span>
    <div class="advisory-body advisory-suggested">${parts.join(' &nbsp;·&nbsp; ')}</div>
    <div class="advisory-rationale">${rec.rationale}</div>
  </div>`;
}

// ── Agent grid renderer ───────────────────────────────────────────────────────

function renderAgents(agents) {
  const grid    = document.getElementById('agent-grid');
  const countEl = document.getElementById('agent-count');
  const ids     = Object.keys(agents || {});

  countEl.textContent = ids.length > 0
    ? `${ids.length} agent${ids.length !== 1 ? 's' : ''}`
    : '—';

  if (ids.length === 0) {
    grid.innerHTML = `
      <div class="empty-state">
        <p>No agent data yet</p>
        <p style="margin-top:10px;font-size:12px;color:var(--subtle)">
          Send <code>POST /request</code> with <code>agent_id</code> and <code>input</code>
          to populate this view.
        </p>
      </div>`;
    return;
  }

  grid.innerHTML = ids.map((id) => {
    const a    = agents[id];
    const rl   = a.rateLimiter  || {};
    const q    = a.queue        || {};
    const conc = a.concurrency  || { inFlight: 0, maxConcurrent: 10 };
    const cfg  = rl.config      || { capacity: 10, refillRatePerSec: 2, critical: false, minTokensReserved: 0 };
    const st   = agentStatus(a);

    const tokenPct    = cfg.capacity > 0
      ? Math.min(100, ((rl.tokens ?? 0) / cfg.capacity) * 100) : 0;
    const qPct        = q.maxQueueLength > 0
      ? Math.min(100, ((q.depth ?? 0) / q.maxQueueLength) * 100) : 0;
    const inflightPct = conc.maxConcurrent > 0
      ? Math.min(100, (conc.inFlight / conc.maxConcurrent) * 100) : 0;

    const errorCls    = (a.errorCount > 0) ? ' danger' : '';
    const droppedMeta = a.dropped > 0
      ? ` &nbsp;·&nbsp; <span style="color:var(--red-d);font-weight:600">${a.dropped} dropped</span>`
      : '';

    const isFairness  = conc.maxConcurrent !== 10 || cfg.critical;
    const fairnessTag = isFairness
      ? `<span class="tag tag-fairness">Fairness</span>` : '';
    const criticalTag = cfg.critical
      ? `<span class="tag tag-critical">Critical</span>` : '';
    const inflightAtLimit = conc.inFlight >= conc.maxConcurrent;

    const reserveMeta = cfg.minTokensReserved > 0
      ? ` &nbsp;·&nbsp; ${cfg.minTokensReserved} reserved` : '';

    return `<div class="agent-card s-${st.cls}" data-agent-id="${id}">

  <div class="card-head">
    <div>
      <span class="agent-name">${id}</span>
      ${criticalTag || fairnessTag
        ? `<div class="tag-row">${criticalTag}${fairnessTag}</div>` : ''}
    </div>
    <span class="s-chip ${st.cls}">
      <span class="s-dot"></span>${st.label}
    </span>
  </div>

  <div class="card-body">

    <div class="mini-stats">
      <div class="ms">
        <div class="ms-lbl">Requests</div>
        <div class="ms-val">${fmt(a.total)}</div>
      </div>
      <div class="ms">
        <div class="ms-lbl">Executed</div>
        <div class="ms-val">${fmt(a.executed)}</div>
      </div>
      <div class="ms">
        <div class="ms-lbl">Queued</div>
        <div class="ms-val">${fmt(a.queued)}</div>
      </div>
      <div class="ms">
        <div class="ms-lbl">Errors</div>
        <div class="ms-val${errorCls}">${fmt(a.errorCount)}</div>
      </div>
    </div>

    <div class="p95-row">
      <span class="p95-lbl">p95 Latency</span>
      <span class="p95-val">${fmtMs(a.p95LatencyMs)}</span>
    </div>

    ${renderSloRow(a)}

    <div class="prog">
      <div class="prog-hd">
        <span class="prog-name">In-Flight</span>
        <span class="inflight-meta">
          ${inflightAtLimit
            ? `<span class="at-limit">${conc.inFlight} / ${conc.maxConcurrent} — at limit</span>`
            : `${conc.inFlight} / ${conc.maxConcurrent}`}
        </span>
      </div>
      <div class="prog-track">
        <div class="prog-fill"
             style="width:${inflightPct.toFixed(1)}%;background-color:${inflightColor(inflightPct)}">
        </div>
      </div>
    </div>

    <div class="prog">
      <div class="prog-hd">
        <span class="prog-name">Rate Limiter</span>
        <span class="prog-meta">
          ${(rl.tokens ?? 0).toFixed(1)} / ${cfg.capacity} tokens
          &nbsp;·&nbsp; ${cfg.refillRatePerSec}/sec${reserveMeta}
        </span>
      </div>
      <div class="prog-track">
        <div class="prog-fill"
             style="width:${tokenPct.toFixed(1)}%;background-color:${tokenColor(tokenPct)}">
        </div>
      </div>
    </div>

    <div class="prog">
      <div class="prog-hd">
        <span class="prog-name">Queue</span>
        <span class="prog-meta">
          ${fmt(q.depth)} / ${fmt(q.maxQueueLength)} depth${droppedMeta}
        </span>
      </div>
      <div class="prog-track">
        <div class="prog-fill"
             style="width:${qPct.toFixed(1)}%;background-color:${queueColor(qPct)}">
        </div>
      </div>
    </div>

    <div class="dedupe-row">
      <span class="dedupe-lbl">Workflow Dedupe</span>
      <span class="dedupe-val${(a.workflowDedupeHits ?? 0) === 0 ? ' zero' : ''}">
        ${fmt(a.workflowDedupeHits ?? 0)} hit${(a.workflowDedupeHits ?? 0) !== 1 ? 's' : ''}
      </span>
    </div>

    ${renderAdvisory(id)}

    <div class="card-hint">Click to configure this agent ↓</div>

  </div>
</div>`;
  }).join('');

  // Click card → pre-fill config form + smooth scroll
  grid.querySelectorAll('.agent-card').forEach((card) => {
    card.addEventListener('click', () => {
      const field = document.getElementById('cfg-id');
      field.value = card.dataset.agentId || '';
      field.focus();
      document.querySelector('.config-panel')
        .scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    });
  });
}

// ── Fetch & poll ──────────────────────────────────────────────────────────────

async function fetchAll() {
  try {
    // All three fetches run in parallel; recommendations failure never blocks the main update.
    const [cacheSettled, agentsSettled, recsSettled] = await Promise.allSettled([
      fetch('/ops/cache/stats'),
      fetch('/ops/agents/stats'),
      fetch('/ops/agents/recommendations'),
    ]);

    if (cacheSettled.status !== 'fulfilled' || !cacheSettled.value.ok ||
        agentsSettled.status !== 'fulfilled' || !agentsSettled.value.ok) {
      throw new Error('Stats fetch failed');
    }

    const cacheData  = await cacheSettled.value.json();
    const agentsData = await agentsSettled.value.json();

    // Update recommendations map — silently ignored if endpoint is unavailable
    if (recsSettled.status === 'fulfilled' && recsSettled.value.ok) {
      const recsData = await recsSettled.value.json();
      latestRecs = {};
      for (const rec of (recsData.agents || [])) {
        latestRecs[rec.agentId] = rec;
      }
    }

    renderStats(cacheData, agentsData.global || {});
    renderAgents(agentsData.agents || {});

    document.getElementById('last-updated').textContent =
      'Updated ' + new Date().toLocaleTimeString();
  } catch (err) {
    document.getElementById('last-updated').textContent =
      'Error — ' + (err.message || 'check console');
  }
}

// ── Config form ───────────────────────────────────────────────────────────────

document.getElementById('cfg-submit').addEventListener('click', async () => {
  const agentId    = document.getElementById('cfg-id').value.trim();
  const capacity   = document.getElementById('cfg-capacity').value.trim();
  const refill     = document.getElementById('cfg-refill').value.trim();
  const queueLen   = document.getElementById('cfg-queue').value.trim();
  const concurrent = document.getElementById('cfg-concurrent').value.trim();
  const reserve    = document.getElementById('cfg-reserve').value.trim();
  const critical   = document.getElementById('cfg-critical').checked;
  const sloP95     = document.getElementById('cfg-slo-p95').value.trim();
  const sloLabel   = document.getElementById('cfg-slo-label').value.trim();
  const msgEl      = document.getElementById('cfg-msg');

  if (!agentId) {
    msgEl.textContent = 'Agent ID is required.';
    msgEl.className = 'err';
    return;
  }
  // At least one numeric field, checkbox, or SLO field must be set
  const criticalChanged = document.getElementById('cfg-critical').dataset.touched === 'true';
  if (!capacity && !refill && !queueLen && !concurrent && !reserve && !criticalChanged && !sloP95 && !sloLabel) {
    msgEl.textContent = 'Provide at least one field to update.';
    msgEl.className = 'err';
    return;
  }

  const body = { agent_id: agentId };
  if (capacity)   body.capacity         = Number(capacity);
  if (refill)     body.refillRatePerSec = Number(refill);
  if (queueLen)   body.maxQueueLength   = Number(queueLen);
  if (concurrent)  body.maxConcurrent     = Number(concurrent);
  if (reserve)     body.minTokensReserved = Number(reserve);
  if (criticalChanged) body.critical     = critical;
  if (sloP95)      body.sloP95Ms         = Number(sloP95);
  if (sloLabel)    body.sloLabel         = sloLabel;

  try {
    const res  = await fetch('/ops/agents/config', {
      method:  'POST',
      headers: { 'Content-Type': 'application/json' },
      body:    JSON.stringify(body),
    });
    const data = await res.json();

    if (!res.ok) {
      msgEl.textContent = 'Error: ' + (data.error || res.statusText);
      msgEl.className = 'err';
    } else {
      msgEl.textContent = `✓ Config applied to "${agentId}"`;
      msgEl.className = 'ok';
      setTimeout(() => { msgEl.textContent = ''; }, 3000);
      fetchAll();
    }
  } catch (err) {
    msgEl.textContent = 'Network error: ' + (err.message || 'unknown');
    msgEl.className = 'err';
  }
});

// Mark the critical checkbox as intentionally changed (distinguishes unchecked from never-set)
document.getElementById('cfg-critical').addEventListener('change', function () {
  this.dataset.touched = 'true';
});

// ── Replay burst ──────────────────────────────────────────────────────────────

function showToast(msg) {
  const toast = document.getElementById('replay-toast');
  toast.textContent = msg;
  toast.classList.add('visible');
  clearTimeout(toast._timer);
  toast._timer = setTimeout(() => toast.classList.remove('visible'), 4500);
}

document.getElementById('replay-btn').addEventListener('click', async () => {
  const btn   = document.getElementById('replay-btn');
  const label = btn.querySelector('.replay-label');

  btn.disabled = true;
  btn.classList.add('running');
  label.textContent = 'Running…';

  try {
    const res  = await fetch('/ops/load/replay', { method: 'POST' });
    const data = await res.json();

    if (!res.ok) {
      showToast('Replay failed: ' + (data.error || res.statusText));
      return;
    }

    const total  = data.totalRequests ?? 0;
    const drops  = (data.replaySummary ?? []).reduce((a, s) => a + (s.dropped  ?? 0), 0);
    const errors = (data.replaySummary ?? []).reduce((a, s) => a + (s.errored  ?? 0), 0);
    const cached = (data.replaySummary ?? []).reduce((a, s) => a + (s.cached   ?? 0), 0);
    const dur    = data.durationMs != null ? `${(data.durationMs / 1000).toFixed(1)} s` : '';

    showToast(
      `Burst replayed ${dur ? `(${dur})` : ''} — ` +
      `${total} req · ${cached} cached · ${drops} dropped · ${errors} errors`,
    );

    fetchAll(); // pull fresh metrics immediately without waiting for next poll
  } catch (err) {
    showToast('Replay failed: ' + (err.message || 'network error'));
  } finally {
    btn.disabled = false;
    btn.classList.remove('running');
    label.textContent = '⚡ Replay Burst';
  }
});

// ── Boot ──────────────────────────────────────────────────────────────────────

fetchAll();
setInterval(fetchAll, POLL_MS);
