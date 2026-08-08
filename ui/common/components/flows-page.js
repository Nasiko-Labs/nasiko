import '/common/components/smart-table.js';
import '/common/components/app-module-nav.js';

import styles from './flows-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { completed: 'success', running: 'info', failed: 'error', timeout: 'warning' };

const statusPill = (v) =>
  v ? `<span class="badge badge--${STATUS_VARIANTS[v] || 'neutral'}"><span class="badge__dot"></span>${v}</span>` : '';

const fmtDuration = (v) => v != null ? (v < 1000 ? `${v}ms` : `${(v / 1000).toFixed(1)}s`) : '—';

class FlowsPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.innerHTML = `
      <app-module-nav module="observability"></app-module-nav>
      <header class="page-head">
        <div>
          <h1 class="title-page">Flows</h1>
          <p class="page-sub">Agent orchestration traces and multi-step flow history.</p>
        </div>
      </header>
      <smart-table
        data-fn="fetchFlows"
        search
        search-placeholder="Search flows by agent or query..."
        limit="20"
      ></smart-table>
    `;

    const table = this.querySelector('smart-table');
    table.columns = [
      { key: 'flow_id', label: 'Flow ID', width: '12%', render: (v) => v ? `<a class="cell-id" href="/flow.html?id=${v}">${v.slice(0, 12)}</a>` : '' },
      { key: 'title', label: 'Query', width: '30%', wrap: true, render: (v) => v ? (v.length > 80 ? v.slice(0, 80) + '...' : v) : '' },
      { key: 'root_agent_name', label: 'Agent', width: '13%', render: (v) => v ? `<span class="cell-agent">${v}</span>` : '' },
      { key: 'status', label: 'Status', width: '11%', render: statusPill },
      { key: 'total_invocations', label: 'Calls', width: '7%', render: (v) => `<span class="cell-num">${v ?? ''}</span>` },
      { key: 'duration_ms', label: 'Duration', width: '9%', render: (v) => `<span class="cell-num">${fmtDuration(v)}</span>` },
      { key: 'created_at', label: 'Started', width: '12%', render: (v) => v ? `<span class="cell-num">${new Date(v).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</span>` : '' },
      { key: 'flow_id', label: '', width: '6%', render: (v) => v ? `<a class="cell-action" href="/flow.html?id=${v}">Trace →</a>` : '' },
    ];
  }
}

customElements.define('flows-page', FlowsPage);
