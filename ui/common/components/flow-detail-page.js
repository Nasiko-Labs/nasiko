import { icons } from '/common/utils/icons.js';

import styles from './flow-detail-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { completed: 'success', running: 'info', failed: 'error', timeout: 'warning' };

class FlowDetailPage extends HTMLElement {
  #initialized = false;

  #toolbar(sub = '') {
    return `<header class="page-head">
      <div>
        <h1 class="title-page">Flow detail</h1>
        ${sub ? `<p class="page-sub">${sub}</p>` : ''}
      </div>
      <a class="back-link" href="/sessions.html?view=flows">${icons.chevronLeft('', 16)}Back to flows</a>
    </header>`;
  }

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    const flowId = new URLSearchParams(location.search).get('id');
    if (!flowId) {
      this.innerHTML = `${this.#toolbar()}
        <div class="empty-state">
          <div class="empty-tile">${icons.activity('', 20)}</div>
          <div class="empty-title">No flow selected</div>
          <p class="empty-sub">Open a flow from the list to inspect its trace.</p>
        </div>`;
      return;
    }
    this.innerHTML = `${this.#toolbar()}<div class="skel-block" style="height:200px"></div>`;
    this.#load(flowId);
  }

  async #load(flowId) {
    const data = await window.fetchFlowDetail(flowId);
    if (!data) {
      this.innerHTML = `${this.#toolbar()}
        <div class="empty-state">
          <div class="empty-tile">${icons.faceFrown('', 20)}</div>
          <div class="empty-title">Flow not found</div>
          <p class="empty-sub">This flow may have expired or been removed.</p>
        </div>`;
      return;
    }

    const flow = data.flow || data;
    const steps = data.steps || [];

    document.title = `Nasiko — Flow ${flow.flow_id.slice(0, 12)}`;
    const variant = STATUS_VARIANTS[flow.status] || 'neutral';
    const duration = flow.duration_ms != null
      ? (flow.duration_ms < 1000 ? `${flow.duration_ms}ms` : `${(flow.duration_ms / 1000).toFixed(1)}s`)
      : 'In progress';
    const started = flow.created_at
      ? new Date(flow.created_at).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
      : '—';

    this.innerHTML = `${this.#toolbar(this.#esc(flow.title || ''))}
      <div class="kpi-strip">
        <div class="kpi">
          <div class="kpi-label">Status</div>
          <div class="kpi-value"><span class="badge badge--${variant}"><span class="badge__dot"></span>${flow.status}</span></div>
        </div>
        <div class="kpi">
          <div class="kpi-label">Root agent</div>
          <div class="kpi-value is-mono">${this.#esc(flow.root_agent_name || 'orchestrator')}</div>
        </div>
        <div class="kpi">
          <div class="kpi-label">Duration</div>
          <div class="kpi-value is-mono">${duration}</div>
        </div>
        <div class="kpi">
          <div class="kpi-label">Started</div>
          <div class="kpi-value is-mono">${started}</div>
        </div>
        <div class="kpi">
          <div class="kpi-label">Steps</div>
          <div class="kpi-value is-mono">${steps.length}</div>
        </div>
      </div>

      <div class="section-head">
        <h2 class="section-title">Steps</h2>
        <p class="section-sub">Each agent call in this flow, in execution order. Expand a step for its input and timing.</p>
      </div>
      <div class="steps" id="trace-container"></div>
    `;

    this.#renderTrace(steps, flow.duration_ms || 1);
  }

  #renderTrace(steps, totalDuration) {
    const container = this.querySelector('#trace-container');
    if (!steps.length) {
      container.innerHTML = `
        <div class="empty-state">
          <div class="empty-tile">${icons.activity('', 20)}</div>
          <div class="empty-title">No steps recorded</div>
          <p class="empty-sub">No agent calls were recorded for this flow.</p>
        </div>`;
      return;
    }

    const flowStart = new Date(steps[0].created_at).getTime();

    container.innerHTML = steps.map((step, i) => {
      const stepStart = new Date(step.created_at).getTime();
      const stepEnd = step.completed_at ? new Date(step.completed_at).getTime() : stepStart + (totalDuration || 1000);
      const offsetPct = ((stepStart - flowStart) / totalDuration) * 100;
      const widthPct = Math.max(2, ((stepEnd - stepStart) / totalDuration) * 100);
      const latency = step.latency_ms != null ? `${step.latency_ms}ms` : (step.completed_at ? `${stepEnd - stepStart}ms` : '…');
      const status = step.status || 'running';
      const numClass = status === 'completed' ? 'done' : status === 'failed' ? 'failed' : 'active';
      const startedAt = new Date(step.created_at).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });

      return `
        <details class="step">
          <summary class="step-row">
            <span class="step-num ${numClass}">${i + 1}</span>
            <span class="step-agent">${this.#esc(step.agent_name)}</span>
            ${step.input_summary ? `<span class="step-snippet">${this.#esc(step.input_summary.slice(0, 60))}${step.input_summary.length > 60 ? '…' : ''}</span>` : '<span class="step-snippet"></span>'}
            <span class="badge badge--${STATUS_VARIANTS[status] || 'neutral'}"><span class="badge__dot"></span>${status}</span>
            <span class="step-latency">${latency}</span>
            <span class="step-caret">${icons.chevronDown('', 14)}</span>
          </summary>
          <div class="step-body">
            ${step.input_summary ? `
              <div class="step-well">
                <div class="well-label">Input</div>
                <div class="well-text">${this.#esc(step.input_summary)}</div>
              </div>` : ''}
            <div class="step-meta">
              <span class="meta-item">Started <span class="is-mono">${startedAt}</span></span>
              <span class="meta-item">Latency <span class="is-mono">${latency}</span></span>
            </div>
            <div class="step-track">
              <div class="step-bar is-${numClass}" style="left:${offsetPct}%;width:${widthPct}%;"></div>
            </div>
          </div>
        </details>
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
