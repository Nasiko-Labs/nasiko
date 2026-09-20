/**
 * Workflow detail — review/edit one MAF workflow, run it, and watch runs.
 *
 * Views (single 720px column, mirroring the mockup's review screen):
 * - review: editable name/description/steps (PUT /api/maf/workflow/{id}),
 *   output_generation display, run button, execution history.
 * - run: live per-step timeline for one execution — polls
 *   GET /api/maf/execution/{id} every 1.5s while pending/running (no SSE).
 *
 * Deep link: /workflow.html?id=<workflow>&exec=<execution>.
 *
 * @element workflow-detail-page
 */
import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { timeAgo } from '/common/utils/date-utils.js';
import { fmtDuration, fmtTokens } from '/common/utils/units.js';
import { renderMarkdown } from '/common/utils/markdown.js';
import '/common/components/app-button.js';
import '/common/components/wf-step-editor.js';
import '/common/components/wf-run-steps.js';

import styles from './workflow-detail-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const POLL_MS = 1500;
const EXEC_BADGES = { success: 'badge--success', failed: 'badge--error', running: 'badge--warning', pending: 'badge--neutral' };

class WorkflowDetailPage extends HTMLElement {
  #initialized = false;
  #workflowId = null;
  #workflow = null;
  #executions = [];
  #execution = null;
  #pollTimer = null;
  #dirty = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    const params = new URLSearchParams(location.search);
    this.#workflowId = params.get('id');
    window.addEventListener('popstate', this.#onPopState);

    if (!this.#workflowId) {
      this.innerHTML = `
        <div class="col">
          <div class="not-found">
            <span class="empty-tile">${icons.workflow('', 24)}</span>
            <p class="not-found-title">No workflow selected</p>
            <p class="not-found-sub">Open a workflow from the <a href="/index.html?view=workflows">library</a>.</p>
          </div>
        </div>`;
      return;
    }
    // Set by the create screen's "Save & run" when the save succeeded but the
    // run request didn't — otherwise the workflow just silently sits unrun.
    const runError = params.get('run_error');
    if (runError) showToast(`Saved, but the run didn't start: ${runError}`);

    this.#load(params.get('exec'));
  }

  disconnectedCallback() {
    this.#stopPolling();
    window.removeEventListener('popstate', this.#onPopState);
  }

  #onPopState = () => {
    const exec = new URLSearchParams(location.search).get('exec');
    if (exec) this.#openRun(exec, { push: false });
    else this.#showReview();
  };

  async #load(execId) {
    try {
      const [workflow, executions] = await Promise.all([
        window.fetchWorkflow(this.#workflowId),
        window.fetchWorkflowExecutions(this.#workflowId).catch(() => []),
      ]);
      this.#workflow = workflow;
      this.#executions = executions;
      document.title = `Nasiko — ${workflow.name}`;
    } catch {
      this.innerHTML = `
        <div class="col">
          <div class="not-found">
            <span class="empty-tile">${icons.faceFrown('', 24)}</span>
            <p class="not-found-title">Workflow not found</p>
            <p class="not-found-sub">It may have been deleted. Back to the
              <a href="/index.html?view=workflows">library</a>.</p>
          </div>
        </div>`;
      return;
    }
    if (execId) this.#openRun(execId, { push: false });
    else this.#showReview();
  }

  // ── Review view ───────────────────────────────────────────────────────────

  #stepLabels() {
    const labels = {};
    for (const s of this.#workflow.maf_json?.steps || []) labels[s.step_id] = s.task_description;
    return labels;
  }

  #showReview() {
    this.#stopPolling();
    this.#execution = null;
    this.#dirty = false;
    const wf = this.#workflow;
    const steps = wf.maf_json?.steps || [];
    const runs = wf.execution_count === 1 ? '1 run' : `${wf.execution_count} runs`;

    this.innerHTML = `
      <div class="col">
        <header class="page-head">
          <a class="back-btn" href="/index.html?view=workflows" aria-label="Back to workflows">${icons.chevronLeft('', 16)}</a>
          <input class="name-input" id="wf-name" value="${this.#esc(wf.name)}" aria-label="Workflow name" />
          <app-button variant="primary" size="sm" id="run-btn">${icons.play('', 12)} Run</app-button>
        </header>

        <textarea class="desc-input" id="wf-desc" rows="2"
          placeholder="Describe what this workflow is for">${this.#esc(wf.description || wf.maf_json?.description || '')}</textarea>

        <div class="badges">
          <span class="badge badge--muted">${steps.length === 1 ? '1 step' : `${steps.length} steps`}</span>
          <span class="badge badge--muted">${this.#esc(runs)}</span>
        </div>

        <section class="sec">
          <h2 class="sec-title">Steps</h2>
          <wf-step-editor id="editor"></wf-step-editor>
        </section>

        ${wf.maf_json?.output_generation ? `
          <section class="sec">
            <h2 class="sec-title">Output guidelines</h2>
            <p class="output-gen">${this.#esc(wf.maf_json.output_generation)}</p>
          </section>` : ''}

        <div class="save-bar" id="save-bar" hidden>
          <span class="save-note">Unsaved changes</span>
          <app-button variant="ghost" size="sm" id="discard-btn">Discard</app-button>
          <app-button variant="primary" size="sm" id="save-btn">Save changes</app-button>
        </div>

        <section class="sec">
          <h2 class="sec-title">Executions</h2>
          <div id="exec-list"></div>
        </section>
      </div>
    `;

    const editor = this.querySelector('#editor');
    editor.steps = steps.map((s) => ({
      taskDescription: s.task_description,
      agentId: s.agent_id,
      agentName: s.agent_name,
    }));
    this.#loadAgents();

    const markDirty = () => {
      this.#dirty = true;
      this.querySelector('#save-bar').hidden = false;
    };
    editor.addEventListener('wf-steps-change', markDirty);
    this.querySelector('#wf-name').addEventListener('input', markDirty);
    this.querySelector('#wf-desc').addEventListener('input', markDirty);

    this.querySelector('#run-btn').addEventListener('click', () => this.#run());
    this.querySelector('#save-btn').addEventListener('click', () => this.#saveEdits());
    this.querySelector('#discard-btn').addEventListener('click', () => this.#showReview());
    this.#renderExecList();
  }

  async #loadAgents() {
    try {
      const res = await apiFetch('/agents?limit=100');
      if (!res.ok) return;
      const body = await res.json();
      const editor = this.querySelector('#editor');
      if (editor) {
        editor.agents = (Array.isArray(body) ? body : body.data || [])
          .map((a) => ({ id: a.id, name: a.display_name || a.name || a.id }));
      }
    } catch { /* picker falls back to the persisted agent names */ }
  }

  #renderExecList() {
    const list = this.querySelector('#exec-list');
    if (!list) return;
    if (!this.#executions.length) {
      list.innerHTML = `<p class="exec-empty">This workflow hasn't run yet. Hit Run to start the
        first execution.</p>`;
      return;
    }
    list.innerHTML = this.#executions.map((e) => `
      <button type="button" class="exec-row" data-exec="${this.#esc(e.id)}">
        <span class="exec-num">#${e.execution_number}</span>
        <span class="badge ${EXEC_BADGES[e.status] || 'badge--neutral'}"><span class="badge__dot"></span>${this.#esc(e.status)}</span>
        <span class="exec-meta">${this.#esc(timeAgo(e.created_at))}</span>
        <span class="exec-meta">${e.duration_ms != null ? fmtDuration(e.duration_ms) : ''}</span>
        <span class="exec-meta">${fmtTokens(e.tokens_used)}</span>
        <span class="exec-open">${icons.chevronRight('', 14)}</span>
      </button>`).join('');
    list.addEventListener('click', (e) => {
      const row = e.target.closest('[data-exec]');
      if (row) this.#openRun(row.dataset.exec, { push: true });
    });
  }

  async #saveEdits() {
    const btn = this.querySelector('#save-btn');
    const steps = this.querySelector('#editor').steps
      .map((s, i) => ({
        step_index: i,
        task_description: s.taskDescription.trim(),
        agent_id: s.agentId || undefined,
      }))
      .filter((s) => s.task_description);
    if (!steps.length) {
      showToast('A workflow needs at least one step with instructions.');
      return;
    }
    btn.setAttribute('loading', '');
    try {
      this.#workflow = await window.updateWorkflow(this.#workflowId, {
        name: this.querySelector('#wf-name').value.trim() || undefined,
        description: this.querySelector('#wf-desc').value.trim() || undefined,
        steps,
      });
      showToast('Workflow updated');
      this.#showReview();
    } catch (err) {
      btn.removeAttribute('loading');
      showToast(`Save failed: ${err.message}`);
    }
  }

  async #run() {
    if (this.#dirty) {
      showToast('Save or discard your edits before running.');
      return;
    }
    const btn = this.querySelector('#run-btn');
    btn?.setAttribute('loading', '');
    try {
      const started = await window.runWorkflow(this.#workflowId);
      this.#openRun(started.execution_id, { push: true });
    } catch (err) {
      btn?.removeAttribute('loading');
      showToast(`Run failed: ${err.message}`);
    }
  }

  // ── Run view ──────────────────────────────────────────────────────────────

  async #openRun(execId, { push }) {
    this.#stopPolling();
    if (push) {
      const url = `/workflow.html?id=${encodeURIComponent(this.#workflowId)}&exec=${encodeURIComponent(execId)}`;
      history.pushState({}, '', url);
    }
    this.#renderRunShell();
    try {
      this.#execution = await window.fetchExecution(execId);
    } catch (err) {
      this.querySelector('#run-body').innerHTML =
        `<p class="exec-empty">Failed to load execution: ${this.#esc(err.message)}</p>`;
      return;
    }
    this.#updateRunView();
    this.#pollIfActive();
  }

  #renderRunShell() {
    this.innerHTML = `
      <div class="col">
        <header class="page-head">
          <button type="button" class="back-btn" id="run-back" aria-label="Back to workflow">${icons.chevronLeft('', 16)}</button>
          <h1 class="title-page run-title" id="run-title">${this.#esc(this.#workflow?.name || 'Execution')}</h1>
        </header>
        <div id="run-body">
          <div class="run-head-row">
            <span class="run-exec-num" id="run-num"></span>
            <span id="run-status"></span>
          </div>
          <div class="badges" id="run-badges"></div>
          <wf-run-steps id="run-steps"></wf-run-steps>
          <section class="sec" id="run-output" hidden>
            <h2 class="sec-title">Output</h2>
            <div class="run-output-body md-body" id="run-output-body"></div>
          </section>
          <div class="run-error" id="run-error" hidden></div>
        </div>
      </div>`;
    this.querySelector('#run-back').addEventListener('click', async () => {
      history.pushState({}, '', `/workflow.html?id=${encodeURIComponent(this.#workflowId)}`);
      // A run just happened — refresh the history list before showing it.
      this.#executions = await window.fetchWorkflowExecutions(this.#workflowId).catch(() => this.#executions);
      this.#showReview();
    });
  }

  #updateRunView() {
    const exec = this.#execution;
    const statusCls = EXEC_BADGES[exec.status] || 'badge--neutral';
    this.querySelector('#run-num').textContent = `Execution #${exec.execution_number}`;
    this.querySelector('#run-status').innerHTML =
      `<span class="badge ${statusCls}"><span class="badge__dot"></span>${this.#esc(exec.status)}</span>`;

    const stepCount = exec.step_results?.length || 0;
    const attempts = exec.attempt_count > 1 ? `attempt ${exec.attempt_count}/${exec.max_attempts}` : '';
    this.querySelector('#run-badges').innerHTML = [
      stepCount ? `${stepCount === 1 ? '1 step' : `${stepCount} steps`}` : '',
      exec.started_at ? `Started ${timeAgo(exec.started_at)}` : '',
      exec.duration_ms != null ? fmtDuration(exec.duration_ms) : '',
      fmtTokens(exec.tokens_used),
      attempts,
    ].filter(Boolean).map((label) => `<span class="badge badge--muted">${this.#esc(label)}</span>`).join('');

    const stepsEl = this.querySelector('#run-steps');
    stepsEl.labels = this.#stepLabels();
    stepsEl.steps = exec.step_results || [];

    const outputSec = this.querySelector('#run-output');
    if (exec.output) {
      outputSec.hidden = false;
      this.querySelector('#run-output-body').innerHTML = renderMarkdown(exec.output);
    } else {
      outputSec.hidden = true;
    }

    const errorEl = this.querySelector('#run-error');
    if (exec.status === 'failed' && exec.error) {
      errorEl.hidden = false;
      errorEl.textContent = exec.error;
    } else {
      errorEl.hidden = true;
    }
  }

  #pollIfActive() {
    const status = this.#execution?.status;
    if (status !== 'pending' && status !== 'running') return;
    this.#pollTimer = setTimeout(async () => {
      try {
        this.#execution = await window.fetchExecution(this.#execution.id);
        if (this.querySelector('#run-body')) this.#updateRunView();
      } catch { /* transient poll failure — keep trying */ }
      this.#pollIfActive();
    }, POLL_MS);
  }

  #stopPolling() {
    clearTimeout(this.#pollTimer);
    this.#pollTimer = null;
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s ?? '';
    return d.innerHTML;
  }
}

customElements.define('workflow-detail-page', WorkflowDetailPage);
