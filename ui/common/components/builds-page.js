import '/common/components/smart-table.js';

const STATUS_VARIANTS = { success: 'success', building: 'info', failed: 'error', queued: 'neutral', cancelled: 'warning' };

class BuildsPage extends HTMLElement {
  #evtSource = null;

  connectedCallback() {
    this.innerHTML = `
      <smart-table
        id="builds-table"
        data-fn="fetchBuilds"
        search
        search-placeholder="Search builds by agent or image..."
        limit="15"
      ></smart-table>
    `;

    const table = this.querySelector('#builds-table');
    table.columns = [
      { key: 'build_id', label: 'Build', width: '14%', render: (v) => `<a href="/build.html?id=${v}" style="color:var(--color-primary);text-decoration:none;font-family:var(--font-mono);font-size:var(--font-size-xs);">#${v.slice(0, 8)}</a>` },
      { key: 'agent_name', label: 'Agent', width: '16%', render: (v) => `<span style="font-weight:500;">${v}</span>` },
      { key: 'image', label: 'Image', width: '20%', render: (v) => `<code style="font-size:var(--font-size-xs);background:var(--color-bg-base);padding:2px 6px;border-radius:var(--radius-sm);">${v}</code>` },
      { key: 'status', label: 'Status', width: '12%', render: (v) => `<app-badge variant="${STATUS_VARIANTS[v] || 'neutral'}">${v}</app-badge>` },
      { key: 'progress', label: 'Progress', width: '14%', render: (v, row) => {
        if (row.status === 'success') return '<span style="color:var(--color-success);">Done</span>';
        if (row.status === 'failed' || row.status === 'cancelled') return '—';
        const pct = v || 0;
        return `<div style="display:flex;align-items:center;gap:var(--space-xs);"><div style="flex:1;height:6px;background:var(--color-border);border-radius:3px;overflow:hidden;"><div style="height:100%;width:${pct}%;background:var(--color-primary);border-radius:3px;transition:width 0.3s;"></div></div><span style="font-size:var(--font-size-xs);color:var(--color-text-muted);min-width:3ch;">${pct}%</span></div>`;
      }},
      { key: 'duration_s', label: 'Duration', width: '10%', render: (v) => v != null ? (v < 60 ? `${v}s` : `${Math.floor(v/60)}m ${v%60}s`) : '—' },
      { key: 'started_at', label: 'Started', width: '14%', render: (v) => v ? new Date(v).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : '—' },
    ];

    this.#connectSSE(table);
  }

  disconnectedCallback() {
    this.#evtSource?.close();
  }

  #connectSSE(table) {
    if (this.#evtSource) this.#evtSource.close();
    this.#evtSource = new EventSource('/api/builds/events');
    this.#evtSource.addEventListener('build-progress', () => table.refresh());
    this.#evtSource.addEventListener('build-complete', () => table.refresh());
    this.#evtSource.onerror = () => {
      this.#evtSource.close();
      setTimeout(() => this.#connectSSE(table), 5000);
    };
  }
}

customElements.define('builds-page', BuildsPage);
