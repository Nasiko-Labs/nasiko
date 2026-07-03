import styles from './usage-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class UsagePage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <h1 class="page-title">Usage</h1>
      <div class="stats" id="stats-grid">
        ${Array.from({ length: 4 }, () => `
          <div class="stat-card">
            <div style="width:60%;height:0.7em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
            <div style="width:40%;height:1.5em;background:var(--color-border);border-radius:var(--radius-sm);margin-top:var(--space-sm);"></div>
          </div>
        `).join('')}
      </div>

      <h2 class="section-title">Daily Usage (7 days)</h2>
      <div class="chart-wrap" id="chart">
        <div style="display:flex;align-items:flex-end;gap:var(--space-xs);height:160px;padding:var(--space-md);">
          ${Array.from({ length: 7 }, (_, i) => `<div style="flex:1;background:var(--color-border);border-radius:var(--radius-sm) var(--radius-sm) 0 0;height:${20 + ((i * 17 + 13) % 60)}%;opacity:0.5;"></div>`).join('')}
        </div>
      </div>

      <div class="tabs">
        <button class="tab-btn is-active" data-tab="agent">By Agent</button>
        <button class="tab-btn" data-tab="model">By Model</button>
      </div>
      <smart-table id="usage-table" data-fn="fetchUsageByAgent" search search-placeholder="Search..." limit="10"></smart-table>
    `;

    this.querySelector('.tabs').addEventListener('click', (e) => {
      const btn = e.target.closest('.tab-btn');
      if (!btn) return;
      this.querySelectorAll('.tab-btn').forEach(t => t.classList.remove('is-active'));
      btn.classList.add('is-active');
      const table = this.querySelector('#usage-table');
      const fn = btn.dataset.tab === 'model' ? 'fetchUsageByModel' : 'fetchUsageByAgent';
      table.setAttribute('data-fn', fn);
    });

    this.#loadSummary();
    this.#loadChart();
    this.#setupTable();
  }

  async #loadSummary() {
    const s = await window.fetchUsageSummary();
    if (!s) return;
    const grid = this.querySelector('#stats-grid');
    grid.innerHTML = `
      <div class="stat-card">
        <div class="stat-label">Total Tokens</div>
        <div class="stat-value">${this.#fmtTokens(s.total_tokens)}</div>
        <div class="stat-sub">${this.#fmtTokens(s.total_input_tokens)} in / ${this.#fmtTokens(s.total_output_tokens)} out</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Total Cost</div>
        <div class="stat-value">${this.#fmtCost(s.total_cost_usd)}</div>
        <div class="stat-sub">Last ${s.period_days || 30} days</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Requests</div>
        <div class="stat-value">${(s.request_count || 0).toLocaleString()}</div>
        <div class="stat-sub">${s.avg_latency_ms != null ? Math.round(s.avg_latency_ms) + 'ms avg' : '—'}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Avg Latency</div>
        <div class="stat-value">${s.avg_latency_ms != null ? Math.round(s.avg_latency_ms) + 'ms' : '—'}</div>
        <div class="stat-sub">per request</div>
      </div>
    `;
  }

  async #loadChart() {
    const data = await window.fetchUsageHistory(7);
    const chart = this.querySelector('#chart');
    if (!data || !data.length) {
      chart.innerHTML = '<p style="color:var(--color-text-muted);text-align:center;padding:var(--space-xl);">No usage data yet.</p>';
      return;
    }

    const days = this.#padToWeek(data);
    const max = Math.max(...days.map(d => d.total_tokens || 0));
    const barHeight = 140;
    chart.innerHTML = `
      <div style="display:flex;align-items:flex-end;gap:var(--space-xs);height:${barHeight + 24}px;padding:0 var(--space-sm);">
        ${days.map(d => {
          const tokens = d.total_tokens || 0;
          const h = max > 0 ? Math.max(tokens > 0 ? 4 : 0, Math.round((tokens / max) * barHeight)) : 0;
          return `<div style="flex:1;display:flex;flex-direction:column;align-items:center;justify-content:flex-end;gap:4px;">
            <div style="width:100%;background:${tokens > 0 ? 'var(--color-primary)' : 'var(--color-border)'};border-radius:var(--radius-sm) var(--radius-sm) 0 0;height:${Math.max(2, h)}px;opacity:${tokens > 0 ? 1 : 0.4};"></div>
            <span style="font-size:var(--font-size-xs);color:var(--color-text-muted);white-space:nowrap;">${d.label}</span>
          </div>`;
        }).join('')}
      </div>
    `;
  }

  #padToWeek(data) {
    const today = new Date();
    const days = [];
    const lookup = new Map(data.map(d => [d.date, d]));
    for (let i = 6; i >= 0; i--) {
      const d = new Date(today);
      d.setDate(d.getDate() - i);
      const key = d.toISOString().slice(0, 10);
      const entry = lookup.get(key);
      days.push({
        total_tokens: entry?.total_tokens || 0,
        label: key.slice(5),
      });
    }
    return days;
  }

  #setupTable() {
    const table = this.querySelector('#usage-table');
    table.columns = [
      { key: 'agent_name', label: 'Agent', width: '25%', render: (v) => v || '(unknown)' },
      { key: 'request_count', label: 'Requests', width: '15%' },
      { key: 'total_tokens', label: 'Tokens', width: '20%', render: (v) => this.#fmtTokens(v) },
      { key: 'total_cost_usd', label: 'Cost', width: '15%', render: (v) => this.#fmtCost(v) },
      { key: 'avg_latency_ms', label: 'Avg Latency', width: '25%', render: (v) => v != null ? `${Math.round(v)}ms` : '—' },
    ];
  }

  #fmtTokens(n) {
    if (n == null) return '0';
    if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + 'M';
    if (n >= 1_000) return (n / 1_000).toFixed(1) + 'K';
    return n.toLocaleString();
  }

  #fmtCost(n) {
    if (n == null) return '$0.00';
    return '$' + n.toFixed(2);
  }
}

customElements.define('usage-page', UsagePage);
