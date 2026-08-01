/**
 * TokenOps dashboard — token-first cost/usage analytics per agent.
 *
 * @element tokenops-page
 * @note Data source: `window.fetchTokenopsDashboard(startTime?)` →
 *       GET /api/observability/finops/dashboard (see /api/docs), which returns
 *       `{ data: { summary, agents, token_usage }, status_code, message }`.
 */
import styles from './tokenops-page.css' with { type: 'css' };
import { icons } from '../utils/icons.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const COLUMNS = [
  { key: 'agent_name', label: 'Agent' },
  { key: 'total_tokens', label: 'Tokens' },
  { key: 'prompt_tokens', label: 'Input' },
  { key: 'completion_tokens', label: 'Output' },
  { key: 'cache_tokens', label: 'Cache r/w' },
  { key: 'operations', label: 'Operations' },
  { key: 'total_cost', label: 'Total cost' },
  { key: 'avg_cost_per_operation', label: 'Avg cost/op' },
  { key: 'container_hours', label: 'Agent hours' },
  { key: 'avg_latency_ms', label: 'Avg latency' },
  { key: 'version', label: 'Version' },
];

const SORTS = [
  { key: 'total_tokens', label: 'Most tokens' },
  { key: 'prompt_tokens', label: 'Most input tokens' },
  { key: 'completion_tokens', label: 'Most output tokens' },
  { key: 'total_cost', label: 'Highest cost' },
  { key: 'operations', label: 'Most operations' },
  { key: 'avg_latency_ms', label: 'Slowest' },
  { key: 'agent_name', label: 'Name' },
];

class TokenopsPage extends HTMLElement {
  #initialized = false;
  #agents = [];
  #query = '';
  #sort = 'total_tokens';

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <div class="page-head">
        <h1 class="page-title">TokenOps dashboard</h1>
        <button class="btn-dark" id="export-btn" type="button">Export</button>
      </div>

      <div class="kpi-strip" id="kpi-strip">${this.#skelKpis(5)}</div>

      <h2 class="section-title">Agent cost</h2>
      <div class="toolbar">
        <div class="search-wrap">
          ${icons.search('', 16)}
          <input type="search" id="agent-search"
            placeholder="Search agents by name, skill, or capability..." />
        </div>
        <div class="toolbar-spacer"></div>
        <select id="month-select" aria-label="Period">${this.#monthOptions()}</select>
        <select id="sort-select" aria-label="Sort">
          ${SORTS.map((s) => `<option value="${s.key}">${s.label}</option>`).join('')}
        </select>
      </div>
      <div class="cost-table-wrap">
        <table>
          <thead>
            <tr>${COLUMNS.map((c) => `<th>${c.label}</th>`).join('')}</tr>
          </thead>
          <tbody id="cost-tbody">
            <tr class="empty-row"><td colspan="${COLUMNS.length}">Loading…</td></tr>
          </tbody>
        </table>
      </div>

      <h2 class="section-title">Token usage</h2>
      <div class="kpi-strip" id="token-strip">${this.#skelKpis(6)}</div>
    `;

    this.querySelector('#agent-search').addEventListener('input', (e) => {
      this.#query = e.target.value.trim().toLowerCase();
      this.#renderTable();
    });
    this.querySelector('#sort-select').addEventListener('change', (e) => {
      this.#sort = e.target.value;
      this.#renderTable();
    });
    this.querySelector('#month-select').addEventListener('change', () => this.#load());
    this.querySelector('#export-btn').addEventListener('click', () => this.#exportCsv());

    this.#load();
  }

  #skelKpis(n) {
    return Array.from({ length: n }, () => `
      <div class="kpi">
        <div class="skel-line skel-line--label"></div>
        <div class="skel-line skel-line--value"></div>
      </div>
    `).join('');
  }

  /** Current + previous 5 months, most recent first. */
  #monthOptions() {
    const fmt = new Intl.DateTimeFormat('en', { month: 'long', year: 'numeric' });
    const now = new Date();
    return Array.from({ length: 6 }, (_, i) => {
      const d = new Date(now.getFullYear(), now.getMonth() - i, 1);
      const value = d.toISOString();
      return `<option value="${value}">${fmt.format(d)}</option>`;
    }).join('');
  }

  async #load() {
    const startTime = this.querySelector('#month-select').value;
    let resp;
    try {
      resp = await window.fetchTokenopsDashboard(startTime);
    } catch (e) {
      console.error('TokenOps dashboard fetch failed:', e);
      this.querySelector('#cost-tbody').innerHTML =
        `<tr class="empty-row"><td colspan="${COLUMNS.length}">Failed to load dashboard</td></tr>`;
      return;
    }
    const data = resp?.data ?? resp ?? {};
    this.#agents = data.agents || [];
    this.#renderSummary(data.summary || {});
    this.#renderTokenUsage(data.token_usage || {});
    this.#renderTable();
  }

  #renderSummary(s) {
    this.querySelector('#kpi-strip').innerHTML = `
      ${this.#kpi('Total tokens', this.#fmtTokens(this.#totalTokens()), 'across all agents')}
      ${this.#kpi('Total operations', (s.total_operations ?? 0).toLocaleString(),
        `${(s.operations_last_24h ?? 0).toLocaleString()} in the last 24 hours`)}
      ${this.#kpi('Agent hours', `${this.#fmtNum(s.total_container_hours)} hrs`, 'Total active execution time')}
      ${this.#kpi('Total cost', this.#fmtCost(s.total_cost),
        `Based on ${(s.total_operations ?? 0).toLocaleString()} operations`)}
      ${this.#kpi('Active agents', `${s.active_agents ?? 0}`,
        `${s.total_agents ?? 0} total agents configured`)}
    `;
  }

  #renderTokenUsage(t) {
    const avg = t.avg_tokens_per_operation ?? 0;
    this.querySelector('#token-strip').innerHTML = `
      ${this.#kpi('Total tokens', this.#fmtTokens(t.total_tokens), '')}
      ${this.#kpi('Input tokens', this.#fmtTokens(t.prompt_tokens), '')}
      ${this.#kpi('Output tokens', this.#fmtTokens(t.completion_tokens), '')}
      ${this.#kpi('Cache read tokens', this.#fmtTokens(t.cache_read_tokens), 'Prompt tokens served from provider cache')}
      ${this.#kpi('Cache write tokens', this.#fmtTokens(t.cache_creation_tokens), 'Prompt tokens written to provider cache')}
      ${this.#kpi('Average tokens / operation', this.#fmtTokens(avg), '')}
    `;
  }

  #kpi(label, value, sub) {
    return `
      <div class="kpi">
        <div class="kpi-label">${label}</div>
        <div class="kpi-value">${value}</div>
        ${sub ? `<div class="kpi-sub">${sub}</div>` : ''}
      </div>
    `;
  }

  #visibleAgents() {
    const rows = this.#query
      ? this.#agents.filter((a) => (a.agent_name || '').toLowerCase().includes(this.#query))
      : [...this.#agents];
    const key = this.#sort;
    rows.sort((a, b) => key === 'agent_name'
      ? (a.agent_name || '').localeCompare(b.agent_name || '')
      : (b[key] ?? 0) - (a[key] ?? 0));
    return rows;
  }

  #renderTable() {
    const tbody = this.querySelector('#cost-tbody');
    const rows = this.#visibleAgents();
    if (!rows.length) {
      tbody.innerHTML = `<tr class="empty-row"><td colspan="${COLUMNS.length}">No agent activity in this period</td></tr>`;
      return;
    }
    tbody.innerHTML = rows.map((a) => `
      <tr>
        <td class="agent-name">${this.#esc(a.agent_name || a.agent_id)}</td>
        <td>${this.#fmtTokens(a.total_tokens)}</td>
        <td>${this.#fmtTokens(a.prompt_tokens)}</td>
        <td>${this.#fmtTokens(a.completion_tokens)}</td>
        <td>${this.#fmtTokens(a.cache_read_tokens)} / ${this.#fmtTokens(a.cache_creation_tokens)}</td>
        <td>${(a.operations ?? 0).toLocaleString()}</td>
        <td>${this.#fmtCost(a.total_cost)}</td>
        <td>${this.#fmtCost(a.avg_cost_per_operation)}</td>
        <td>${this.#fmtNum(a.container_hours)} hrs</td>
        <td>${this.#fmtLatency(a.avg_latency_ms)}</td>
        <td>${this.#esc(a.version || '—')}</td>
      </tr>
    `).join('');
  }

  #exportCsv() {
    const header = COLUMNS.map((c) => c.label).join(',');
    const lines = this.#visibleAgents().map((a) => [
      a.agent_name, a.total_tokens ?? 0, a.prompt_tokens ?? 0, a.completion_tokens ?? 0,
      `${a.cache_read_tokens ?? 0} / ${a.cache_creation_tokens ?? 0}`,
      a.operations ?? 0, a.total_cost ?? 0,
      a.avg_cost_per_operation ?? 0, a.container_hours ?? 0,
      a.avg_latency_ms ?? '', a.version ?? '',
    ].map((v) => `"${String(v).replaceAll('"', '""')}"`).join(','));
    const blob = new Blob([[header, ...lines].join('\n')], { type: 'text/csv' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'tokenops.csv';
    a.click();
    URL.revokeObjectURL(a.href);
  }

  #totalTokens() {
    return this.#agents.reduce((sum, a) => sum + (a.total_tokens ?? 0), 0);
  }

  #fmtTokens(n) {
    if (n == null) return '0';
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 10_000) return `${(n / 1_000).toFixed(1)}K`;
    return n.toLocaleString();
  }

  #fmtCost(n) {
    return `$${(n ?? 0).toFixed(3)}`;
  }

  #fmtNum(n) {
    return (n ?? 0).toFixed(1);
  }

  #fmtLatency(ms) {
    return ms == null ? '—' : `${(ms / 1000).toFixed(1)}s`;
  }

  #esc(str) {
    if (!str) return '';
    return String(str).replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('tokenops-page', TokenopsPage);
