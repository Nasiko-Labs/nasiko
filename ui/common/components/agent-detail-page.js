import { icons } from '/common/utils/icons.js';
import { fetchApi } from '/common/services/api.js';
import { connectSSE } from '/common/services/sse.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (agent-detail-page) {
  :scope { display: block; }
  .header {
    display: flex;
    align-items: center;
    gap: var(--space-md);
    margin-bottom: var(--space-xl);
  }
  .header-info { flex: 1; }
  .header-name {
    font-size: var(--font-size-2xl);
    font-weight: 600;
    color: var(--color-text-main);
  }
  .header-meta {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin-top: var(--space-xs);
    display: flex;
    gap: var(--space-md);
    align-items: center;
  }
  .header-actions {
    display: flex;
    gap: var(--space-sm);
  }
  .action-btn {
    padding: var(--space-xs) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    cursor: pointer;
  }
  .action-btn:hover { border-color: var(--color-primary); }
  .action-btn.is-primary {
    border-color: var(--color-primary);
    background: color-mix(in srgb, var(--color-primary) 10%, transparent);
  }
  .tabs {
    display: flex;
    gap: var(--space-md);
    border-bottom: 1px solid var(--color-border);
    margin-bottom: var(--space-lg);
  }
  .tab {
    padding: var(--space-sm) var(--space-xs);
    font-size: var(--font-size-sm);
    font-weight: 500;
    color: var(--color-text-muted);
    border: none;
    border-bottom: 2px solid transparent;
    background: none;
    cursor: pointer;
  }
  .tab:hover { color: var(--color-text-main); }
  .tab.is-active { color: var(--color-primary); border-bottom-color: var(--color-primary); }
  .panel { display: none; }
  .panel.is-active { display: block; }
  .info-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: var(--space-md);
    margin-bottom: var(--space-xl);
  }
  .info-card {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    background: var(--color-bg-surface);
  }
  .info-label {
    font-size: var(--font-size-xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--color-text-muted);
  }
  .info-value {
    font-size: var(--font-size-base);
    color: var(--color-text-main);
    margin-top: var(--space-xs);
    font-weight: 500;
  }
  .tags { display: flex; flex-wrap: wrap; gap: 6px; margin-top: var(--space-sm); }
  .tag {
    font-size: var(--font-size-xs);
    padding: 2px 10px;
    border-radius: var(--radius-sm);
    background: var(--color-bg-base);
    border: 1px solid var(--color-border);
    color: var(--color-text-muted);
  }
  .skills-list {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: var(--space-sm);
  }
  .skill-item {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-sm) var(--space-md);
    background: var(--color-bg-surface);
  }
  .skill-name { font-weight: 500; font-size: var(--font-size-sm); }
  .skill-desc { font-size: var(--font-size-xs); color: var(--color-text-muted); margin-top: 2px; }

  /* ── Log viewer ──────────────────────────────────────────────────────────── */
  .log-toolbar {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    margin-bottom: var(--space-sm);
    flex-wrap: wrap;
  }
  .log-toolbar input, .log-toolbar select {
    padding: 4px 8px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-xs);
  }
  .log-toolbar input { flex: 1; min-width: 160px; }
  .log-follow-btn {
    padding: 4px 12px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-sm);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-xs);
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .log-follow-btn.is-active {
    border-color: var(--color-success, #22c55e);
    color: var(--color-success, #22c55e);
    background: color-mix(in srgb, var(--color-success, #22c55e) 10%, transparent);
  }
  .log-viewer {
    background: var(--color-bg-base);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    font-family: var(--font-mono);
    font-size: var(--font-size-xs);
    line-height: 1.8;
    height: 480px;
    overflow-y: auto;
    white-space: pre-wrap;
    word-break: break-all;
  }
  .log-line { display: flex; gap: var(--space-sm); }
  .log-ts { color: var(--color-text-muted); flex-shrink: 0; }
  .log-level { width: 48px; flex-shrink: 0; font-weight: 600; }
  .log-source { color: var(--color-text-muted); flex-shrink: 0; font-size: 10px; }
  .log-msg { flex: 1; }
  .log-level-INFO  { color: var(--color-primary, #6366f1); }
  .log-level-WARN  { color: var(--color-warning, #f59e0b); }
  .log-level-ERROR { color: var(--color-error, #ef4444); }
  .log-level-DEBUG { color: var(--color-text-muted); }

  /* ── Stats panel ─────────────────────────────────────────────────────────── */
  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: var(--space-md);
    margin-bottom: var(--space-xl);
  }
  .stat-card {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    background: var(--color-bg-surface);
  }
  .stat-label {
    font-size: var(--font-size-xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--color-text-muted);
  }
  .stat-value {
    font-size: var(--font-size-xl);
    font-weight: 700;
    color: var(--color-text-main);
    margin-top: var(--space-xs);
  }
  .stat-sub {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    margin-top: 2px;
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TABS = [
  { key: 'overview', label: 'Overview' },
  { key: 'skills',   label: 'Skills'   },
  { key: 'logs',     label: 'Logs'     },
  { key: 'metrics',  label: 'Metrics'  },
  { key: 'flows',    label: 'Flows'    },
];

const LEVEL_COLORS = { INFO: '#6366f1', WARN: '#f59e0b', ERROR: '#ef4444', DEBUG: '#9ca3af' };

class AgentDetailPage extends HTMLElement {
  #agent    = null;
  #agentId  = null;
  #sseSource = null;   // active EventSource for log tail

  connectedCallback() {
    this.#agentId = new URLSearchParams(location.search).get('id');
    if (!this.#agentId) {
      this.innerHTML = '<p style="color:var(--color-text-muted);">No agent ID specified.</p>';
      return;
    }
    this.innerHTML = '<app-skeleton height="200px"></app-skeleton>';
    this.#load();
  }

  disconnectedCallback() {
    this.#stopFollow();
  }

  async #load() {
    try {
      // The catalog endpoint accepts UUID or name and returns the full agent row.
      this.#agent = await fetchApi(`/catalog/agents/${this.#agentId}`);
    } catch {
      this.#agent = null;
    }
    if (!this.#agent) {
      this.innerHTML = '<p style="color:var(--color-error);">Agent not found.</p>';
      return;
    }
    document.title = `Nasiko — ${this.#agent.display_name || this.#agent.name}`;
    this.#render();
  }

  #render() {
    const a = this.#agent;
    this.innerHTML = `
      <div class="header">
        <div class="header-info">
          <div class="header-name">${a.display_name || a.name}</div>
          <div class="header-meta">
            <code style="font-size:var(--font-size-xs);background:var(--color-bg-base);padding:2px 6px;border-radius:var(--radius-sm);">${a.image || ''}</code>
            <span>v${a.version || '?'}</span>
            <app-badge variant="${a.status === 'running' ? 'success' : 'warning'}">${a.status || 'unknown'}</app-badge>
          </div>
        </div>
        <div class="header-actions">
          <a class="action-btn is-primary" href="/chat.html?agent_id=${a.id}">Chat</a>
          <a class="action-btn" href="/agent-card.html?id=${a.id}">View Card</a>
        </div>
      </div>

      <nav class="tabs">${TABS.map(t =>
        `<button class="tab${t.key === 'overview' ? ' is-active' : ''}" data-tab="${t.key}">${t.label}</button>`
      ).join('')}</nav>

      <div class="panel is-active" data-panel="overview">
        <div class="info-grid">
          <div class="info-card"><div class="info-label">Description</div><div class="info-value" style="font-weight:400;">${a.description || '—'}</div></div>
          <div class="info-card"><div class="info-label">Status</div><div class="info-value"><app-badge variant="${a.status === 'running' ? 'success' : 'warning'}">${a.status}</app-badge></div></div>
          <div class="info-card"><div class="info-label">Port</div><div class="info-value">${a.port || '—'}</div></div>
          <div class="info-card"><div class="info-label">Version</div><div class="info-value">${a.version || '—'}</div></div>
        </div>
        ${a.tags?.length ? `<div class="tags">${a.tags.map(t => `<span class="tag">${t}</span>`).join('')}</div>` : ''}
      </div>

      <div class="panel" data-panel="skills">
        <div class="skills-list" id="skills-list">
          ${(a.skills || []).map(s => `
            <div class="skill-item">
              <div class="skill-name">${s.name}</div>
              <div class="skill-desc">${s.description || ''}</div>
            </div>
          `).join('') || '<p style="color:var(--color-text-muted);font-style:italic;">No skills registered.</p>'}
        </div>
      </div>

      <div class="panel" data-panel="logs">
        <div class="log-toolbar">
          <input type="text"   id="log-search"  placeholder="Search messages…">
          <select id="log-level">
            <option value="">All levels</option>
            <option>INFO</option><option>WARN</option><option>ERROR</option><option>DEBUG</option>
          </select>
          <button class="log-follow-btn" id="log-follow-btn" title="Live tail">
            <span id="log-follow-dot">●</span>&nbsp;Follow
          </button>
          <button class="action-btn" id="log-refresh-btn">Refresh</button>
        </div>
        <div class="log-viewer" id="log-viewer">
          <span style="color:var(--color-text-muted);">Loading…</span>
        </div>
      </div>

      <div class="panel" data-panel="metrics">
        <div id="metrics-content"><app-skeleton height="120px"></app-skeleton></div>
      </div>

      <div class="panel" data-panel="flows">
        <p style="color:var(--color-text-muted);">Flow traces involving this agent will appear here once the Tempo observability backend is configured.</p>
      </div>
    `;

    this.querySelector('.tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.tab');
      if (!tab) return;
      this.querySelectorAll('.tab').forEach(t => t.classList.remove('is-active'));
      this.querySelectorAll('.panel').forEach(p => p.classList.remove('is-active'));
      tab.classList.add('is-active');
      this.querySelector(`[data-panel="${tab.dataset.tab}"]`).classList.add('is-active');
      if (tab.dataset.tab === 'logs')    this.#initLogs();
      if (tab.dataset.tab === 'metrics') this.#loadMetrics();
    });

    // Auto-load logs on first render if on the logs tab
    this.#initLogs();
  }

  // ── Logs tab ────────────────────────────────────────────────────────────────

  #initLogs() {
    this.#loadLogs();

    const searchEl = this.querySelector('#log-search');
    const levelEl  = this.querySelector('#log-level');
    const followBtn = this.querySelector('#log-follow-btn');
    const refreshBtn = this.querySelector('#log-refresh-btn');

    if (!searchEl || searchEl.dataset.bound) return;
    searchEl.dataset.bound = '1';

    let debounce;
    searchEl.addEventListener('input', () => {
      clearTimeout(debounce);
      debounce = setTimeout(() => this.#loadLogs(), 350);
    });
    levelEl.addEventListener('change', () => this.#loadLogs());
    refreshBtn.addEventListener('click', () => this.#loadLogs());
    followBtn.addEventListener('click', () => this.#toggleFollow());
  }

  async #loadLogs() {
    const viewer = this.querySelector('#log-viewer');
    if (!viewer) return;

    const search = this.querySelector('#log-search')?.value || '';
    const level  = this.querySelector('#log-level')?.value  || '';

    const params = new URLSearchParams({ limit: '200' });
    if (search) params.set('search', search);
    if (level)  params.set('level',  level);

    viewer.innerHTML = '<span style="color:var(--color-text-muted);">Loading…</span>';

    try {
      const logs = await fetchApi(`/observe/agents/${this.#agentId}/logs?${params}`);
      if (!logs.length) {
        viewer.innerHTML = '<span style="color:var(--color-text-muted);">No logs found.</span>';
        return;
      }
      viewer.innerHTML = logs.map(l => this.#renderLogLine(l)).join('\n');
      viewer.scrollTop = viewer.scrollHeight;
    } catch (err) {
      viewer.innerHTML = `<span style="color:var(--color-error);">Failed to load logs: ${err.message}</span>`;
    }
  }

  #renderLogLine(l) {
    const ts  = l.timestamp ? new Date(l.timestamp).toISOString().replace('T', ' ').slice(0, 23) : '';
    const lvl = (l.level || 'INFO').toUpperCase();
    const src = l.source ? `[${l.source}]` : '';
    const msg = (l.message || '').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    return `<div class="log-line">` +
      `<span class="log-ts">${ts}</span>` +
      `<span class="log-level log-level-${lvl}">${lvl.padEnd(5)}</span>` +
      `<span class="log-source">${src}</span>` +
      `<span class="log-msg">${msg}</span>` +
      `</div>`;
  }

  #toggleFollow() {
    const btn = this.querySelector('#log-follow-btn');
    if (this.#sseSource) {
      this.#stopFollow();
      btn?.classList.remove('is-active');
    } else {
      this.#startFollow();
      btn?.classList.add('is-active');
    }
  }

  #startFollow() {
    const viewer = this.querySelector('#log-viewer');
    if (!viewer) return;
    viewer.innerHTML = '';

    this.#sseSource = connectSSE(`/observe/agents/${this.#agentId}/logs/stream`, {
      onMessage: (line) => {
        if (!line || typeof line !== 'object') return;
        viewer.insertAdjacentHTML('beforeend', this.#renderLogLine(line) + '\n');
        viewer.scrollTop = viewer.scrollHeight;
      },
      onError: () => {
        this.#stopFollow();
        this.querySelector('#log-follow-btn')?.classList.remove('is-active');
      },
    });
  }

  #stopFollow() {
    if (this.#sseSource) {
      this.#sseSource.close();
      this.#sseSource = null;
    }
  }

  // ── Metrics tab ─────────────────────────────────────────────────────────────

  async #loadMetrics() {
    const el = this.querySelector('#metrics-content');
    if (!el) return;

    try {
      const s = await fetchApi(`/observe/agents/${this.#agentId}/stats`);
      const fmt = (n, dec = 0) => (n == null ? '—' : (+n).toFixed(dec));
      el.innerHTML = `
        <div class="stats-grid">
          <div class="stat-card">
            <div class="stat-label">Requests (24 h)</div>
            <div class="stat-value">${fmt(s.total_requests)}</div>
            <div class="stat-sub">since ${new Date(s.period_start).toLocaleDateString()}</div>
          </div>
          <div class="stat-card">
            <div class="stat-label">Error Rate</div>
            <div class="stat-value" style="color:${s.error_rate > 0.1 ? 'var(--color-error)' : 'inherit'}">${fmt(s.error_rate * 100, 1)}%</div>
            <div class="stat-sub">HTTP 4xx / 5xx</div>
          </div>
          <div class="stat-card">
            <div class="stat-label">Avg Latency</div>
            <div class="stat-value">${fmt(s.avg_latency_ms, 0)} ms</div>
            <div class="stat-sub">
              p50 ${s.p50_latency_ms != null ? fmt(s.p50_latency_ms, 0) + ' ms' : '—'} ·
              p95 ${s.p95_latency_ms != null ? fmt(s.p95_latency_ms, 0) + ' ms' : '—'}
            </div>
          </div>
          <div class="stat-card">
            <div class="stat-label">Input Tokens</div>
            <div class="stat-value">${fmt(s.total_input_tokens)}</div>
            <div class="stat-sub">source: ${s.source}</div>
          </div>
          <div class="stat-card">
            <div class="stat-label">Output Tokens</div>
            <div class="stat-value">${fmt(s.total_output_tokens)}</div>
            <div class="stat-sub">source: ${s.source}</div>
          </div>
        </div>
      `;
    } catch (err) {
      el.innerHTML = `<p style="color:var(--color-error);">Could not load metrics: ${err.message}</p>`;
    }
  }
}

customElements.define('agent-detail-page', AgentDetailPage);
