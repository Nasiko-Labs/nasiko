import '/common/components/smart-table.js';

import styles from './builds-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { success: 'success', building: 'info', failed: 'error', queued: 'neutral', cancelled: 'warning' };

const statusPill = (v) =>
  v ? `<span class="badge badge--${STATUS_VARIANTS[v] || 'neutral'}"><span class="badge__dot"></span>${v}</span>` : '';

class BuildsPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.innerHTML = `
      <header class="page-head">
        <div>
          <h1 class="title-page">Builds</h1>
          <p class="page-sub">Agent image build history and progress.</p>
        </div>
      </header>
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
      { key: 'id', label: 'Build', width: '12%', render: (v) => v ? `<a class="cell-id" href="/build.html?id=${v}">#${v.slice(0, 8)}</a>` : '' },
      { key: 'version_tag', label: 'Version', width: '12%', render: (v) => v ? `<span class="cell-num">${v}</span>` : '—' },
      { key: 'image_reference', label: 'Image', width: '26%', render: (v) => v ? `<span class="cell-image">${v}</span>` : '—' },
      { key: 'status', label: 'Status', width: '13%', render: statusPill },
      { key: 'github_url', label: 'Source', width: '13%', render: (v) => v ? `<a class="cell-action" href="${v}" target="_blank" rel="noopener">repo ↗</a>` : '—' },
      { key: 'created_at', label: 'Started', width: '16%', render: (v) => v ? `<span class="cell-num">${new Date(v).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}</span>` : '—' },
      { key: 'id', label: '', width: '8%', render: (v) => v ? `<a class="cell-action" href="/build.html?id=${v}">Logs →</a>` : '' },
    ];
  }
}

customElements.define('builds-page', BuildsPage);
