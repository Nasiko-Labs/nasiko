const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (build-detail-page) {
  :scope { display: block; }
  .steps {
    display: flex;
    flex-direction: column;
    gap: var(--space-sm);
    margin-bottom: var(--space-xl);
  }
  .step {
    display: flex;
    align-items: center;
    gap: var(--space-md);
    padding: var(--space-sm) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
  }
  .step-num {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: var(--font-size-xs);
    font-weight: 700;
    flex-shrink: 0;
  }
  .step-num.done { background: var(--color-success-bg); color: var(--color-success); }
  .step-num.active { background: var(--color-primary); color: white; }
  .step-num.pending { background: var(--color-bg-base); color: var(--color-text-muted); }
  .step-num.failed { background: var(--color-error-bg); color: var(--color-error); }
  .step-name { font-size: var(--font-size-sm); font-weight: 500; flex: 1; }
  .step-duration { font-size: var(--font-size-xs); color: var(--color-text-muted); }
  .log-viewer {
    background: var(--color-bg-base);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    font-family: var(--font-mono);
    font-size: var(--font-size-xs);
    line-height: 1.8;
    max-height: 400px;
    overflow-y: auto;
    white-space: pre-wrap;
    word-break: break-all;
    color: var(--color-text-main);
  }
  .log-line { display: block; }
  .log-line .ts { color: var(--color-text-muted); margin-right: var(--space-sm); }
  .log-line.is-error { color: var(--color-error); }
  .summary-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: var(--space-md);
    margin-bottom: var(--space-xl);
  }
  .summary-card {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    background: var(--color-bg-surface);
  }
  .summary-card-label {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .summary-card-value {
    font-size: var(--font-size-sm);
    margin-top: var(--space-xs);
  }
  .summary-card-value code {
    font-family: var(--font-mono);
    font-size: var(--font-size-xs);
  }
  h2 { font-size: var(--font-size-lg); font-weight: 600; margin-bottom: var(--space-md); }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { success: 'success', building: 'info', failed: 'error', queued: 'neutral', cancelled: 'warning' };

class BuildDetailPage extends HTMLElement {
  #buildId = null;

  connectedCallback() {
    this.#buildId = new URLSearchParams(location.search).get('id');
    if (!this.#buildId) {
      this.innerHTML = '<p style="color:var(--color-text-muted);">No build ID specified.</p>';
      return;
    }
    this.innerHTML = '<app-skeleton height="300px"></app-skeleton>';
    this.#load();
  }

  async #load() {
    const build = await window.fetchBuildDetail(this.#buildId);
    if (!build) {
      this.innerHTML = '<p style="color:var(--color-error);">Build not found.</p>';
      return;
    }

    document.title = `Nasiko — Build #${build.build_id.slice(0, 8)}`;
    const variant = STATUS_VARIANTS[build.status] || 'neutral';

    const stepsHtml = (build.steps || []).map((s, i) => {
      let numClass = 'pending';
      if (s.status === 'done') numClass = 'done';
      else if (s.status === 'active') numClass = 'active';
      else if (s.status === 'failed') numClass = 'failed';
      return `
        <div class="step">
          <div class="step-num ${numClass}">${i + 1}</div>
          <span class="step-name">${s.name}</span>
          <span class="step-duration">${s.duration_s != null ? s.duration_s + 's' : ''}</span>
        </div>
      `;
    }).join('');

    const logsHtml = (build.logs || []).map(l => {
      const cls = l.level === 'error' ? ' is-error' : '';
      return `<span class="log-line${cls}"><span class="ts">${l.ts || ''}</span>${l.msg}</span>`;
    }).join('\n');

    this.innerHTML = `
      <div class="summary-grid">
        <div class="summary-card">
          <div class="summary-card-label">Build ID</div>
          <div class="summary-card-value"><code>#${build.build_id.slice(0, 8)}</code></div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Agent</div>
          <div class="summary-card-value" style="font-weight:500;">${build.agent_name}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Status</div>
          <div class="summary-card-value"><app-badge variant="${variant}">${build.status}</app-badge></div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Image</div>
          <div class="summary-card-value"><code>${build.image}</code></div>
        </div>
      </div>

      ${stepsHtml ? `<h2>Steps</h2><div class="steps">${stepsHtml}</div>` : ''}

      <h2>Logs</h2>
      <div class="log-viewer" id="log-viewer">${logsHtml || '<span style="color:var(--color-text-muted);font-style:italic;">No logs available.</span>'}</div>
    `;

    if (build.status === 'building') this.#connectSSE();
  }

  #connectSSE() {
    const viewer = this.querySelector('#log-viewer');
    const evtSource = new EventSource(`/api/builds/${this.#buildId}/logs`);
    evtSource.addEventListener('log', (e) => {
      try {
        const l = JSON.parse(e.data);
        const cls = l.level === 'error' ? ' is-error' : '';
        viewer.innerHTML += `\n<span class="log-line${cls}"><span class="ts">${l.ts || ''}</span>${l.msg}</span>`;
        viewer.scrollTop = viewer.scrollHeight;
      } catch {}
    });
    evtSource.addEventListener('done', () => {
      evtSource.close();
      this.#load();
    });
    evtSource.onerror = () => evtSource.close();
  }
}

customElements.define('build-detail-page', BuildDetailPage);
