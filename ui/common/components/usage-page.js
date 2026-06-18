import { icons } from '/common/utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (usage-page) {
  :scope { display: block; }
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
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
    font-size: var(--font-size-2xl);
    font-weight: 700;
    color: var(--color-text-main);
    margin-top: var(--space-xs);
  }
  .stat-sub {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    margin-top: 2px;
  }
  .section-title {
    font-size: var(--font-size-lg);
    font-weight: 600;
    color: var(--color-text-main);
    margin-bottom: var(--space-md);
  }
  .chart-wrap {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    background: var(--color-bg-surface);
    margin-bottom: var(--space-xl);
    min-height: 200px;
  }
  .tabs {
    display: flex;
    gap: var(--space-md);
    margin-bottom: var(--space-md);
  }
  .tab-btn {
    padding: var(--space-xs) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    color: var(--color-text-muted);
    font-size: var(--font-size-sm);
    cursor: pointer;
  }
  .tab-btn.is-active {
    border-color: var(--color-primary);
    background: color-mix(in srgb, var(--color-primary) 10%, transparent);
    color: var(--color-text-main);
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class UsagePage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <div class="stats" id="stats-grid">
        ${Array.from({ length: 4 }, () => `
          <div class="stat-card">
            <div style="width:60%;height:0.7em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
            <div style="width:40%;height:1.5em;background:var(--color-border);border-radius:var(--radius-sm);margin-top:var(--space-sm);"></div>
          </div>
        `).join('')}
      </div>

      <h2 class="section-title">Daily Usage (7 days)</h2>
      <div class="chart-wrap" id="chart" style="display:flex;align-items:flex-end;gap:var(--space-xs);height:200px;padding:var(--space-md);">
        ${Array.from({ length: 7 }, () => `<div style="flex:1;background:var(--color-border);border-radius:var(--radius-sm) var(--radius-sm) 0 0;height:${20 + Math.floor(Math.random() * 60)}%;opacity:0.5;"></div>`).join('')}
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
        <div class="stat-label">Today</div>
        <div class="stat-value">${this.#fmtTokens(s.today_tokens)}</div>
        <div class="stat-sub">${this.#fmtCost(s.today_cost)}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">This Week</div>
        <div class="stat-value">${this.#fmtTokens(s.week_tokens)}</div>
        <div class="stat-sub">${this.#fmtCost(s.week_cost)}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">This Month</div>
        <div class="stat-value">${this.#fmtTokens(s.month_tokens)}</div>
        <div class="stat-sub">${this.#fmtCost(s.month_cost)}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Total Requests</div>
        <div class="stat-value">${(s.total_requests || 0).toLocaleString()}</div>
        <div class="stat-sub">${s.active_agents || 0} active agents</div>
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
    const max = Math.max(...data.map(d => d.tokens));
    chart.innerHTML = `
      <div style="display:flex;align-items:flex-end;gap:var(--space-xs);height:160px;">
        ${data.map(d => {
          const pct = max > 0 ? (d.tokens / max) * 100 : 0;
          return `<div style="flex:1;display:flex;flex-direction:column;align-items:center;gap:4px;">
            <div style="width:100%;background:var(--color-primary);border-radius:var(--radius-sm) var(--radius-sm) 0 0;height:${pct}%;min-height:2px;transition:height 0.3s;"></div>
            <span style="font-size:var(--font-size-xs);color:var(--color-text-muted);">${d.date?.slice(5) || ''}</span>
          </div>`;
        }).join('')}
      </div>
    `;
  }

  #setupTable() {
    const table = this.querySelector('#usage-table');
    table.columns = [
      { key: 'name', label: 'Name', width: '30%' },
      { key: 'requests', label: 'Requests', width: '15%' },
      { key: 'tokens', label: 'Tokens', width: '20%', render: (v) => this.#fmtTokens(v) },
      { key: 'cost', label: 'Cost', width: '15%', render: (v) => this.#fmtCost(v) },
      { key: 'avg_latency_ms', label: 'Avg Latency', width: '20%', render: (v) => v != null ? `${v}ms` : '—' },
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
