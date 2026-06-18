import { icons } from '/common/utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (flow-detail-page) {
  :scope { display: block; }
  .page-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-lg);
  }
  .page-toolbar h1 { font-size: var(--font-size-xl); font-weight: 600; margin: 0; }
  .back-link {
    display: inline-flex;
    align-items: center;
    gap: var(--space-xs);
    font-size: var(--font-size-sm);
    color: var(--color-primary);
    text-decoration: none;
  }
  .back-link:hover { text-decoration: underline; }
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
  h2 { font-size: var(--font-size-lg); font-weight: 600; margin-bottom: var(--space-md); }
  .trace {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    padding: var(--space-md);
    overflow-x: auto;
  }
  .trace-span {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    padding: var(--space-xs) 0;
    border-bottom: 1px solid var(--color-border-subtle, rgba(0,0,0,0.05));
  }
  .trace-span:last-child { border-bottom: none; }
  .trace-agent {
    font-size: var(--font-size-sm);
    font-weight: 500;
    min-width: 140px;
    flex-shrink: 0;
  }
  .trace-bar-container {
    flex: 1;
    height: 20px;
    position: relative;
    background: var(--color-bg-base);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .trace-bar {
    position: absolute;
    top: 2px;
    bottom: 2px;
    border-radius: var(--radius-sm);
    min-width: 4px;
  }
  .trace-bar.completed { background: var(--color-primary); opacity: 0.8; }
  .trace-bar.failed { background: var(--color-error); opacity: 0.8; }
  .trace-bar.running { background: var(--color-primary); opacity: 0.5; }
  .trace-duration {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    min-width: 60px;
    text-align: right;
    flex-shrink: 0;
  }
  .trace-status {
    min-width: 24px;
    flex-shrink: 0;
  }
  .trace-input {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    margin-top: 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 300px;
  }
  .no-steps {
    color: var(--color-text-muted);
    font-style: italic;
    padding: var(--space-md);
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { completed: 'success', running: 'info', failed: 'error', timeout: 'warning' };

class FlowDetailPage extends HTMLElement {
  #toolbar = `<div class="page-toolbar">
    <h1>Flow Detail</h1>
    <a class="back-link" href="/flows.html">${icons.chevronLeft('', 16)}Back to Flows</a>
  </div>`;

  connectedCallback() {
    const flowId = new URLSearchParams(location.search).get('id');
    if (!flowId) {
      this.innerHTML = `${this.#toolbar}<p style="color:var(--color-text-muted);">No flow ID specified.</p>`;
      return;
    }
    this.innerHTML = `${this.#toolbar}<app-skeleton height="200px"></app-skeleton>`;
    this.#load(flowId);
  }

  async #load(flowId) {
    const data = await window.fetchFlowDetail(flowId);
    if (!data) {
      this.innerHTML = `${this.#toolbar}<p style="color:var(--color-error);">Flow not found.</p>`;
      return;
    }

    const flow = data.flow || data;
    const steps = data.steps || [];

    document.title = `Nasiko — Flow ${flow.flow_id.slice(0, 12)}`;
    const variant = STATUS_VARIANTS[flow.status] || 'neutral';
    const duration = flow.duration_ms != null
      ? (flow.duration_ms < 1000 ? `${flow.duration_ms}ms` : `${(flow.duration_ms / 1000).toFixed(1)}s`)
      : 'In progress';
    const started = flow.created_at ? new Date(flow.created_at).toLocaleString() : '';

    const agents = [...new Set(steps.map(s => s.agent_name))];

    this.innerHTML = `${this.#toolbar}
      <div class="summary-grid">
        <div class="summary-card">
          <div class="summary-card-label">Status</div>
          <div class="summary-card-value"><app-badge variant="${variant}">${flow.status}</app-badge></div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Root Agent</div>
          <div class="summary-card-value">${flow.root_agent_name || 'orchestrator'}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Duration</div>
          <div class="summary-card-value">${duration}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Started</div>
          <div class="summary-card-value">${started}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Steps</div>
          <div class="summary-card-value">${steps.length}</div>
        </div>
        <div class="summary-card">
          <div class="summary-card-label">Query</div>
          <div class="summary-card-value" style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis;" title="${this.#esc(flow.title || '')}">${flow.title || ''}</div>
        </div>
      </div>

      <h2>Trace</h2>
      <div class="trace" id="trace-container"></div>
    `;

    this.#renderTrace(steps, flow.duration_ms || 1);
  }

  #renderTrace(steps, totalDuration) {
    const container = this.querySelector('#trace-container');
    if (!steps.length) {
      container.innerHTML = '<div class="no-steps">No agent calls recorded for this flow.</div>';
      return;
    }

    const flowStart = steps.length > 0 ? new Date(steps[0].created_at).getTime() : 0;

    container.innerHTML = steps.map(step => {
      const stepStart = new Date(step.created_at).getTime();
      const stepEnd = step.completed_at ? new Date(step.completed_at).getTime() : stepStart + (totalDuration || 1000);
      const offsetPct = ((stepStart - flowStart) / totalDuration) * 100;
      const widthPct = Math.max(2, ((stepEnd - stepStart) / totalDuration) * 100);
      const latency = step.latency_ms != null ? `${step.latency_ms}ms` : (step.completed_at ? `${stepEnd - stepStart}ms` : '...');
      const statusClass = step.status || 'running';

      return `
        <div class="trace-span">
          <div class="trace-status"><app-badge variant="${STATUS_VARIANTS[statusClass] || 'neutral'}" style="font-size:9px;">${statusClass}</app-badge></div>
          <div class="trace-agent">
            ${step.agent_name}
            ${step.input_summary ? `<div class="trace-input" title="${this.#esc(step.input_summary)}">${this.#esc(step.input_summary.slice(0, 60))}</div>` : ''}
          </div>
          <div class="trace-bar-container">
            <div class="trace-bar ${statusClass}" style="left:${offsetPct}%;width:${widthPct}%;"></div>
          </div>
          <div class="trace-duration">${latency}</div>
        </div>
      `;
    }).join('');
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('flow-detail-page', FlowDetailPage);
