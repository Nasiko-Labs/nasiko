import { icons } from '/common/utils/icons.js';

import styles from './flow-detail-page.css' with { type: 'css' };
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
