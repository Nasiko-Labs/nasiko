/**
 * Vertical per-step timeline for one workflow execution (MAF step_results).
 *
 * The server snapshots `step_results` on every transition, so re-assigning
 * `steps` while polling GET /api/maf/execution/{id} yields a live timeline.
 *
 * @element wf-run-steps
 * @prop {Array} steps - Raw `step_results` rows ({step_index, agent_name,
 *       status, error, prompt, extracted_info, tokens_used, latency_ms}).
 * @prop {Object} labels - Optional map step_id → task_description, used as
 *       the step title when the caller has the workflow's maf_json handy.
 */
import { icons } from '/common/utils/icons.js';
import { renderMarkdown } from '/common/utils/markdown.js';
import { fmtDuration, fmtTokens } from '/common/utils/units.js';

import styles from './wf-run-steps.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_META = {
  success: { label: 'Complete', cls: 'is-success' },
  failed:  { label: 'Failed',   cls: 'is-failed' },
  running: { label: 'Running',  cls: 'is-running' },
  pending: { label: 'Pending',  cls: 'is-pending' },
};

class WfRunSteps extends HTMLElement {
  #steps = [];
  #labels = {};
  #openTab = {}; // step index → 'output' | 'prompt'

  set steps(value) {
    this.#steps = Array.isArray(value) ? value : [];
    this.#render();
  }

  set labels(value) {
    this.#labels = value || {};
    this.#render();
  }

  connectedCallback() {
    this.addEventListener('click', this.#onClick);
    this.#render();
  }

  disconnectedCallback() {
    this.removeEventListener('click', this.#onClick);
  }

  #onClick = (e) => {
    const pill = e.target.closest('[data-tab]');
    if (!pill) return;
    this.#openTab[pill.dataset.step] = pill.dataset.tab;
    this.#render();
  };

  #statusIcon(status) {
    if (status === 'success') return icons.checkCircle('', 16);
    if (status === 'failed') return icons.xCircle('', 16);
    if (status === 'running') return `<span class="spin">${icons.loader('', 16)}</span>`;
    return icons.circle('', 16);
  }

  #detailHtml(step, i) {
    // Pending/running steps stay compact rows — prompts only matter post-hoc.
    if (step.status !== 'success' && step.status !== 'failed') return '';
    const output = step.extracted_info || '';
    const prompt = step.prompt || '';
    const error = step.error || '';
    if (!output && !prompt && !error) return '';

    const tab = this.#openTab[i] || (output ? 'output' : 'prompt');
    const pills = [];
    if (output) pills.push(['output', 'Output']);
    if (prompt) pills.push(['prompt', 'Prompt']);
    const pillRow = pills.length > 1
      ? `<div class="pane-tabs">${pills.map(([key, label]) =>
          `<button type="button" class="pane-tab${tab === key ? ' is-active' : ''}"
            data-step="${i}" data-tab="${key}">${label}</button>`).join('')}</div>`
      : '';

    let pane = '';
    if (error) {
      pane = `<div class="pane pane--error">${this.#esc(error)}</div>`;
    } else if (tab === 'prompt' && prompt) {
      pane = `<div class="pane pane--mono">${this.#esc(prompt)}</div>`;
    } else if (output) {
      pane = `<div class="pane md-body">${renderMarkdown(output)}</div>`;
    }
    return `<div class="step-detail">${pillRow}${pane}</div>`;
  }

  #render() {
    if (!this.#steps.length) {
      this.innerHTML = '';
      return;
    }
    this.innerHTML = this.#steps.map((step, i) => {
      const meta = STATUS_META[step.status] || STATUS_META.pending;
      const label = this.#labels[step.step_id] || '';
      const title = label
        ? `Step ${i + 1} — ${this.#esc(label)}`
        : `Step ${i + 1} — ${this.#esc(step.agent_name || 'Unassigned')}`;
      const chips = [
        label ? `<span class="meta">${this.#esc(step.agent_name || '')}</span>` : '',
        `<span class="meta status ${meta.cls}">${meta.label}</span>`,
        step.latency_ms ? `<span class="meta">${fmtDuration(step.latency_ms)}</span>` : '',
        step.tokens_used ? `<span class="meta">${fmtTokens(step.tokens_used)}</span>` : '',
      ].filter(Boolean).join('');
      return `
        <div class="step">
          <div class="rail-col ${meta.cls}">
            ${this.#statusIcon(step.status)}
            ${i < this.#steps.length - 1 ? '<span class="rail-line"></span>' : ''}
          </div>
          <div class="step-body">
            <div class="step-head">
              <span class="step-title">${title}</span>
              ${chips}
            </div>
            ${this.#detailHtml(step, i)}
          </div>
        </div>`;
    }).join('');
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s ?? '';
    return d.innerHTML;
  }
}

customElements.define('wf-run-steps', WfRunSteps);
