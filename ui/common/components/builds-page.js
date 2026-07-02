import '/common/components/smart-table.js';

import styles from './builds-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { success: 'success', building: 'info', failed: 'error', queued: 'neutral', cancelled: 'warning' };

class BuildsPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <div class="page-header">
        <h1 class="page-title">Builds</h1>
        <p class="page-desc">Agent image build history and progress.</p>
      </div>
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
      { key: 'id', label: 'Build', width: '14%', render: (v) => `<a href="/build.html?id=${v}" style="color:var(--color-primary);text-decoration:none;font-family:var(--font-mono);font-size:var(--font-size-xs);">#${v ? v.slice(0, 8) : ''}</a>` },
      { key: 'version_tag', label: 'Version', width: '14%', render: (v) => `<span style="font-weight:500;">${v || '—'}</span>` },
      { key: 'image_reference', label: 'Image', width: '22%', render: (v) => v ? `<code style="font-size:var(--font-size-xs);background:var(--color-bg-base);padding:2px 6px;border-radius:var(--radius-sm);">${v}</code>` : '—' },
      { key: 'status', label: 'Status', width: '12%', render: (v) => `<app-badge variant="${STATUS_VARIANTS[v] || 'neutral'}">${v || ''}</app-badge>` },
      { key: 'github_url', label: 'Source', width: '18%', render: (v) => v ? `<a href="${v}" target="_blank" style="color:var(--color-primary);font-size:var(--font-size-xs);text-decoration:none;">repo</a>` : '—' },
      { key: 'created_at', label: 'Started', width: '20%', render: (v) => v ? new Date(v).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : '—' },
    ];

  }
}

customElements.define('builds-page', BuildsPage);
