/**
 * Build detail — one agent image build (record from GET /api/builds/{id}).
 *
 * @element build-detail-page
 * @note Streams status transitions from GET /api/builds/{id}/progress (SSE)
 *       while the build is in flight; raw logs are linked via logs_url.
 */
import { apiFetch } from '/common/services/api.js';
import { connectSSE } from '/common/services/sse.js';
import { icons } from '/common/utils/icons.js';

import styles from './build-detail-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { success: 'success', building: 'info', failed: 'error', queued: 'neutral', cancelled: 'warning', pending: 'neutral' };

class BuildDetailPage extends HTMLElement {
  #initialized = false;
  #buildId = null;
  #evtSource = null;

  #toolbar(sub = '') {
    return `<header class="page-head">
      <div>
        <h1 class="title-page">Build detail</h1>
        ${sub ? `<p class="page-sub">${sub}</p>` : ''}
      </div>
      <a class="back-link" href="/builds.html">${icons.chevronLeft('', 16)}Back to builds</a>
    </header>`;
  }

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#buildId = new URLSearchParams(location.search).get('id');
    if (!this.#buildId) {
      this.innerHTML = `${this.#toolbar()}
        <div class="empty-state">
          <div class="empty-tile">${icons.briefcase('', 20)}</div>
          <div class="empty-title">No build selected</div>
          <p class="empty-sub">Open a build from the list to inspect it.</p>
        </div>`;
      return;
    }
    this.innerHTML = `${this.#toolbar()}<div class="skel-block" style="height:300px"></div>`;
    this.#load();
  }

  disconnectedCallback() {
    this.#evtSource?.close();
    this.#evtSource = null;
  }

  async #load() {
    let build = null;
    try {
      const res = await apiFetch(`/builds/${this.#buildId}`);
      if (res.ok) build = await res.json();
    } catch { /* fall through to not-found */ }
    if (!build) {
      this.innerHTML = `${this.#toolbar()}
        <div class="empty-state">
          <div class="empty-tile">${icons.faceFrown('', 20)}</div>
          <div class="empty-title">Build not found</div>
          <p class="empty-sub">This build may have been pruned or the ID is wrong.</p>
        </div>`;
      return;
    }

    const shortId = String(build.id).slice(0, 8);
    document.title = `Nasiko — Build #${shortId}`;
    const variant = STATUS_VARIANTS[build.status] || 'neutral';
    const fmtTs = (v) => v ? new Date(v).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : '—';

    this.innerHTML = `${this.#toolbar(`<span class="is-mono">#${this.#esc(shortId)}</span> · ${this.#esc(build.image_reference || '')}`)}
      <div class="kpi-strip">
        <div class="kpi">
          <div class="kpi-label">Status</div>
          <div class="kpi-value"><span class="badge badge--${variant}"><span class="badge__dot"></span>${this.#esc(build.status)}</span></div>
        </div>
        <div class="kpi">
          <div class="kpi-label">Version</div>
          <div class="kpi-value is-mono">${this.#esc(build.version_tag || '—')}</div>
        </div>
        <div class="kpi">
          <div class="kpi-label">Started</div>
          <div class="kpi-value is-mono">${fmtTs(build.created_at)}</div>
        </div>
        <div class="kpi">
          <div class="kpi-label">Updated</div>
          <div class="kpi-value is-mono">${fmtTs(build.updated_at)}</div>
        </div>
      </div>

      <h2 class="section-title">Details</h2>
      <div class="detail-rows">
        <div class="detail-row"><span class="detail-key">Image</span><span class="detail-val">${this.#esc(build.image_reference || '—')}</span></div>
        <div class="detail-row"><span class="detail-key">Agent</span><span class="detail-val">${this.#esc(build.agent_id || '—')}</span></div>
        <div class="detail-row"><span class="detail-key">Commit</span><span class="detail-val">${this.#esc(build.commit_hash ? build.commit_hash.slice(0, 12) : '—')}</span></div>
        <div class="detail-row"><span class="detail-key">Source</span><span class="detail-val">${build.github_url ? `<a class="detail-link" href="${this.#esc(build.github_url)}" target="_blank" rel="noopener">${this.#esc(build.github_url)} ↗</a>` : '—'}</span></div>
      </div>

      <div class="section-head">
        <h2 class="section-title">Build log</h2>
        ${build.logs_url ? `<a class="detail-link" href="${this.#esc(build.logs_url)}" target="_blank" rel="noopener">Raw logs ↗</a>` : ''}
      </div>
      <div class="log-viewer" id="log-viewer">${this.#initialLogLines(build)}</div>
    `;

    if (build.status === 'building' || build.status === 'queued' || build.status === 'pending') {
      this.#connectSSE();
    }
  }

  #initialLogLines(build) {
    const lines = [
      `<span class="log-line"><span class="ts">${this.#esc(this.#logTs(build.created_at))}</span>build ${this.#esc(String(build.id))} created</span>`,
      `<span class="log-line"><span class="ts">${this.#esc(this.#logTs(build.updated_at))}</span>status: ${this.#esc(build.status)}</span>`,
    ];
    if (!build.logs_url && (build.status === 'building' || build.status === 'queued' || build.status === 'pending')) {
      lines.push('<span class="log-line is-muted">waiting for status updates…</span>');
    }
    return lines.join('\n');
  }

  #logTs(v) {
    return v ? new Date(v).toLocaleTimeString(undefined, { hour12: false }) : '';
  }

  #connectSSE() {
    const viewer = this.querySelector('#log-viewer');
    // connectSSE (not raw EventSource) so the multi-tenant dashboard streams from
    // the workspace control plane via apiBase; it also JSON-parses each event.
    this.#evtSource = connectSSE(`/builds/${this.#buildId}/progress`, {
      onMessage: (update) => {
        const status = update && update.status;
        if (!status) return;
        const cls = status === 'failed' ? ' is-error' : '';
        viewer.innerHTML += `\n<span class="log-line${cls}"><span class="ts">${this.#esc(new Date().toLocaleTimeString(undefined, { hour12: false }))}</span>status: ${this.#esc(status)}</span>`;
        viewer.scrollTop = viewer.scrollHeight;
        if (status === 'success' || status === 'failed' || status === 'not_found') {
          this.#evtSource?.close();
          this.#evtSource = null;
          if (status !== 'not_found') this.#load();
        }
      },
      onError: () => { this.#evtSource?.close(); this.#evtSource = null; },
    });
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s ?? '';
    return d.innerHTML;
  }
}

customElements.define('build-detail-page', BuildDetailPage);
