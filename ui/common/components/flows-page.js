import '/common/components/smart-table.js';

import styles from './flows-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { completed: 'success', running: 'info', failed: 'error', timeout: 'warning' };

class FlowsPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <div class="page-header">
        <h1 class="page-title">Flows</h1>
        <p class="page-desc">Agent orchestration traces and multi-step flow history.</p>
      </div>
      <smart-table
        data-fn="fetchFlows"
        search
        search-placeholder="Search flows by agent or query..."
        limit="20"
      ></smart-table>
    `;

    const table = this.querySelector('smart-table');
    table.columns = [
      { key: 'flow_id', label: 'Flow ID', width: '14%', render: (v) => `<a href="/flow.html?id=${v}" style="color:var(--color-primary);text-decoration:none;font-family:var(--font-mono);font-size:var(--font-size-xs);">${v ? v.slice(0, 12) : ''}</a>` },
      { key: 'title', label: 'Query', width: '28%', wrap: true, render: (v) => v ? (v.length > 80 ? v.slice(0, 80) + '...' : v) : '' },
      { key: 'root_agent_name', label: 'Agent', width: '14%', render: (v) => v ? `<app-badge variant="neutral">${v}</app-badge>` : '' },
      { key: 'status', label: 'Status', width: '12%', render: (v) => `<app-badge variant="${STATUS_VARIANTS[v] || 'neutral'}">${v || ''}</app-badge>` },
      { key: 'total_invocations', label: 'Calls', width: '8%' },
      { key: 'duration_ms', label: 'Duration', width: '10%', render: (v) => v != null ? (v < 1000 ? `${v}ms` : `${(v / 1000).toFixed(1)}s`) : '' },
      { key: 'created_at', label: 'Started', width: '14%', render: (v) => v ? new Date(v).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : '' },
    ];
  }
}

customElements.define('flows-page', FlowsPage);
