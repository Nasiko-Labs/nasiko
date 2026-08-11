import { icons } from '/common/utils/icons.js';
import '/common/components/app-module-nav.js';
import { showToast } from '/common/utils/toast.js';
import { timeAgo } from '/common/utils/date-utils.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import '/common/components/app-badge.js';
import '/common/components/app-empty-state.js';
import '/common/components/app-skeleton.js';
import '/common/components/smart-table.js';

import styles from './runtime-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

// Exact serde strings of ee InfraStatus (snake_case).
const STATUS_VARIANTS = {
  pending: 'info',
  planning: 'info',
  applying: 'info',
  ready: 'success',
  destroying: 'warning',
  destroyed: 'neutral',
  failed: 'error',
};
const TRANSITIONAL = new Set(['pending', 'planning', 'applying', 'destroying']);
const PROVIDER_LABELS = { digitalocean: 'DigitalOcean', aws: 'AWS', azure: 'Azure' };
const POLL_INTERVAL_MS = 5000;

class RuntimePage extends HTMLElement {
  #initialized = false;
  #clusters = [];
  #listSnapshot = null;
  #pollTimer = null;
  #eventSource = null;
  #detailId = null;

  connectedCallback() {
    if (this.#initialized) {
      this.querySelector('#clusters-table')?.refresh();
      return;
    }
    this.#initialized = true;
    this.#render();
    this.#bind();
  }

  disconnectedCallback() {
    this.#stopPolling();
    this.#closeLogs();
  }

  #render() {
    this.innerHTML = `
      <app-module-nav module="observability"></app-module-nav>
      <div class="page-head">
        <h1 class="title-page">Agent runtime</h1>
        <p class="page-sub">Kubernetes agent-runtime clusters provisioned by <code>nasiko-ee init</code>. Provisioning runs in the background on the control plane — watch status and live logs here.</p>
      </div>
      <smart-table id="clusters-table" limit="15"></smart-table>
      <div id="clusters-empty" hidden>
        <app-empty-state
          title="No agent-runtime clusters yet"
          description="Run nasiko-ee init with the Kubernetes agent runtime to provision one — it will show up here while the control plane provisions it in the background."
        ></app-empty-state>
      </div>

      <app-modal id="cluster-modal" heading="Cluster">
        <div class="cluster-detail">
          <div id="detail-info"></div>
          <h3 class="log-heading">Provisioning Log</h3>
          <div class="log-viewer" id="detail-log"></div>
        </div>
        <div class="form-actions" data-slot="footer">
          <app-button variant="danger" size="sm" id="btn-destroy" hidden>Destroy Cluster</app-button>
          <app-button variant="secondary" size="sm" id="btn-close">Close</app-button>
        </div>
      </app-modal>
    `;

    const table = this.querySelector('#clusters-table');
    table.columns = [
      { key: 'name', label: 'Name', width: '20%', render: (v, row) =>
        `<button class="link-btn" data-action="view" data-id="${this.#esc(row.id)}">${this.#esc(v)}</button>` },
      { key: 'provider', label: 'Provider', width: '12%', render: (v) => this.#esc(PROVIDER_LABELS[v] || v) },
      { key: 'region', label: 'Region', width: '11%', render: (v) => this.#esc(v) },
      { key: 'node_size', label: 'Nodes', width: '15%', render: (v) =>
        v ? `<code>${this.#esc(v)}</code>` : '<span class="muted">--</span>' },
      { key: 'status', label: 'Status', width: '13%', render: (v) => this.#statusBadge(v) },
      { key: 'created_at', label: 'Created', width: '13%', render: (v) => this.#fmtDate(v) },
      { key: 'updated_at', label: 'Updated', width: '10%', render: (v) => v ? this.#esc(timeAgo(v)) : '--' },
      { key: 'actions', label: '', width: '6%', render: (_, row) => {
        if (TRANSITIONAL.has(row.status) || row.status === 'destroyed') return '';
        return `<button class="action-btn action-btn--danger" data-action="destroy"
          data-id="${this.#esc(row.id)}" data-name="${this.#esc(row.name)}" title="Destroy cluster">${icons.trash('', 16)}</button>`;
      } },
    ];

    table.dataFn = async () => {
      const res = await window.fetchInfraClusters();
      this.#onClustersLoaded(res.data || []);
      return res;
    };
    table.refresh();
  }

  #bind() {
    const modal = this.querySelector('#cluster-modal');

    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('[data-action]');
      if (!btn) return;
      const { action, id, name } = btn.dataset;
      if (action === 'view') this.#openDetail(id);
      else if (action === 'destroy') this.#destroy(id, name);
    });

    this.querySelector('#btn-close').addEventListener('click', () => modal.close());
    this.querySelector('#btn-destroy').addEventListener('click', () => {
      const cluster = this.#clusters.find((c) => c.id === this.#detailId);
      if (cluster) this.#destroy(cluster.id, cluster.name);
    });

    // The `close` event fires on the internal <dialog> and does not bubble.
    modal.querySelector('dialog')?.addEventListener('close', () => {
      this.#detailId = null;
      this.#closeLogs();
    });
  }

  // ── List ──────────────────────────────────────────────────────────────────

  #onClustersLoaded(clusters) {
    this.#clusters = clusters;
    this.#listSnapshot = JSON.stringify(clusters);

    const table = this.querySelector('#clusters-table');
    const empty = this.querySelector('#clusters-empty');
    table.style.display = clusters.length ? '' : 'none';
    empty.hidden = clusters.length > 0;

    if (clusters.some((c) => TRANSITIONAL.has(c.status))) this.#startPolling();
    else this.#stopPolling();
  }

  #startPolling() {
    if (this.#pollTimer) return;
    this.#pollTimer = setInterval(() => this.#pollOnce(), POLL_INTERVAL_MS);
  }

  #stopPolling() {
    if (!this.#pollTimer) return;
    clearInterval(this.#pollTimer);
    this.#pollTimer = null;
  }

  // Cheap change detection so the table (and its loading skeletons) only
  // re-render when something actually changed.
  async #pollOnce() {
    let res;
    try { res = await window.fetchInfraClusters(); } catch { return; }
    const data = res.data || [];
    if (JSON.stringify(data) === this.#listSnapshot) return;
    this.querySelector('#clusters-table').refresh();
    if (this.#detailId) this.#refreshDetailInfo();
  }

  // ── Detail ────────────────────────────────────────────────────────────────

  async #openDetail(id) {
    this.#detailId = id;
    const modal = this.querySelector('#cluster-modal');
    const info = this.querySelector('#detail-info');
    info.innerHTML = '<app-skeleton height="120px"></app-skeleton>';
    modal.open();
    await this.#refreshDetailInfo();
    this.#openLogs(id);
  }

  async #refreshDetailInfo() {
    const id = this.#detailId;
    if (!id) return;
    let cluster;
    try {
      cluster = await window.fetchInfraCluster(id);
    } catch (e) {
      this.querySelector('#detail-info').innerHTML =
        `<p class="detail-error-text">Failed to load cluster: ${this.#esc(e.message)}</p>`;
      return;
    }
    if (this.#detailId !== id) return; // detail switched while fetching
    this.#renderDetailInfo(cluster);
  }

  #renderDetailInfo(cluster) {
    const modal = this.querySelector('#cluster-modal');
    modal.setAttribute('heading', cluster.name);

    const errorHtml = cluster.status === 'failed' && cluster.error_message ? `
      <div class="error-banner" role="alert">
        <strong>Provisioning failed</strong>
        <pre>${this.#esc(cluster.error_message)}</pre>
      </div>
    ` : '';

    const outputEntries = cluster.outputs ? Object.entries(cluster.outputs) : [];
    const outputsHtml = outputEntries.length ? `
      <h3 class="log-heading">Outputs</h3>
      <dl class="outputs-list">
        ${outputEntries.map(([k, v]) => `
          <div class="outputs-item">
            <dt>${this.#esc(k)}</dt>
            <dd><code>${this.#esc(v)}</code></dd>
          </div>
        `).join('')}
      </dl>
    ` : '';

    this.querySelector('#detail-info').innerHTML = `
      ${errorHtml}
      <div class="summary-grid">
        <div class="summary-card">
          <div class="summary-card-label">Status</div>
          <div class="summary-card-value">${this.#statusBadge(cluster.status)}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Provider</div>
          <div class="summary-card-value">${this.#esc(PROVIDER_LABELS[cluster.provider] || cluster.provider)}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Region</div>
          <div class="summary-card-value">${this.#esc(cluster.region)}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Nodes</div>
          <div class="summary-card-value">${cluster.node_size ? `<code>${this.#esc(cluster.node_size)}</code>` : '<span class="muted">--</span>'}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Created</div>
          <div class="summary-card-value">${this.#fmtDate(cluster.created_at)}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Updated</div>
          <div class="summary-card-value">${cluster.updated_at ? this.#esc(timeAgo(cluster.updated_at)) : '--'}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Cluster ID</div>
          <div class="summary-card-value"><code>${this.#esc(cluster.id)}</code></div>
        </div>
      </div>
      ${outputsHtml}
    `;

    // Destroy has a 409 guard server-side while a job is active — hide it for
    // transitional statuses (and for already-destroyed clusters).
    const destroyBtn = this.querySelector('#btn-destroy');
    destroyBtn.hidden = TRANSITIONAL.has(cluster.status) || cluster.status === 'destroyed';
  }

  // ── Provisioning log (SSE) ────────────────────────────────────────────────

  #openLogs(id) {
    this.#closeLogs();
    const viewer = this.querySelector('#detail-log');
    viewer.innerHTML = '';
    const es = new EventSource(`/api/infra/clusters/${id}/logs`);
    es.onmessage = (e) => this.#appendLog(viewer, e.data);
    // The stream ends when the provisioning job finishes (or immediately with
    // a one-shot status line when no job is active) — don't auto-reconnect.
    es.onerror = () => es.close();
    this.#eventSource = es;
  }

  #closeLogs() {
    if (!this.#eventSource) return;
    this.#eventSource.close();
    this.#eventSource = null;
  }

  #appendLog(viewer, raw) {
    // The server sends plain-text data events; tolerate JSON-quoted strings.
    let line = raw;
    try {
      const parsed = JSON.parse(raw);
      if (typeof parsed === 'string') line = parsed;
    } catch { /* plain text line */ }
    const span = document.createElement('span');
    span.className = 'log-line';
    span.textContent = line;
    viewer.appendChild(span);
    viewer.scrollTop = viewer.scrollHeight;
  }

  // ── Destroy ───────────────────────────────────────────────────────────────

  async #destroy(id, name) {
    if (!confirm(`Destroy cluster "${name}"? Its cloud resources will be removed. This cannot be undone.`)) return;
    try {
      const res = await window.destroyInfraCluster(id);
      if (!res.ok) throw new Error((await res.text()) || res.statusText);
      showToast('Cluster destroy started');
      this.querySelector('#cluster-modal').close();
      this.querySelector('#clusters-table').refresh();
    } catch (e) {
      showToast(`Failed: ${e.message}`);
    }
  }

  // ── Helpers ───────────────────────────────────────────────────────────────

  #statusBadge(status) {
    const variant = STATUS_VARIANTS[status] || 'neutral';
    const progress = TRANSITIONAL.has(status) ? ' dot class="is-progress"' : '';
    return `<app-badge variant="${variant}"${progress}>${this.#esc(status)}</app-badge>`;
  }

  #fmtDate(v) {
    if (!v) return '--';
    return new Date(v).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
  }

  #esc(value) {
    if (value == null) return '';
    return String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }
}

customElements.define('runtime-page', RuntimePage);
