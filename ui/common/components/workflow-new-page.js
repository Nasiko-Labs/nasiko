/**
 * Create workflow — describe the outcome, let the planner draft steps
 * (POST /api/maf/generate), edit them, then save (POST /api/maf/workflows).
 *
 * The generate call has three designed failure modes: 503 (no OPENAI_API_KEY
 * on the server), 400 (the user has no agents), 422 (planner failure) — each
 * gets a friendly inline notice; manual authoring always stays available.
 *
 * @element workflow-new-page
 */
import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-button.js';
import '/common/components/wf-step-editor.js';

import styles from './workflow-new-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class WorkflowNewPage extends HTMLElement {
  #initialized = false;
  #drafting = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <div class="col">
        <header class="page-head">
          <a class="back-btn" href="/workflows.html" aria-label="Back to workflows">${icons.chevronLeft('', 16)}</a>
          <input class="name-input" id="wf-name" placeholder="Name this workflow" aria-label="Workflow name" />
          <span class="draft-pill">Draft</span>
        </header>

        <section class="describe">
          <div class="describe-head">
            <span class="describe-label">What should this workflow do?</span>
            <button type="button" class="draft-btn" id="draft-btn">${icons.sparkles('', 13)} Draft steps</button>
          </div>
          <textarea id="wf-desc" rows="3"
            placeholder="Describe the outcome you want, in one or two sentences."></textarea>
          <span class="hint">Example: draft three caption variations each weekday, review the tone,
            then queue the approved ones for publishing.</span>
          <div class="gen-notice" id="gen-notice" hidden></div>
        </section>

        <section class="steps-sec">
          <div class="steps-head">
            <span class="steps-label">Steps</span>
            <span class="count-pill" id="step-count">1 step</span>
            <span class="spacer"></span>
            <span class="drafted-note" id="drafted-note" hidden>
              ${icons.sparkles('', 12)} Drafted by Nasiko — edit anything below</span>
          </div>
          <div class="drafting" id="drafting" hidden>
            <div class="drafting-line">
              <span class="spin">${icons.loader('', 16)}</span>
              Reading your description and drafting steps
            </div>
            ${Array.from({ length: 3 }, () => `
              <div class="drafting-card">
                <span class="drafting-bar is-short"></span>
                <span class="drafting-bar"></span>
                <span class="drafting-bar is-mid"></span>
              </div>`).join('')}
          </div>
          <wf-step-editor id="editor"></wf-step-editor>
        </section>

        <hr class="divider" />
        <footer class="foot">
          <span class="footnote" id="footnote"></span>
          <div class="foot-actions">
            <a class="cancel-link" href="/workflows.html">Cancel</a>
            <app-button variant="secondary" size="sm" id="save-btn">Save workflow</app-button>
            <app-button variant="primary" size="sm" id="save-run-btn">${icons.play('', 12)} Save and run</app-button>
          </div>
        </footer>
      </div>
    `;

    const editor = this.querySelector('#editor');
    editor.steps = [{ taskDescription: '', agentId: '', agentName: '' }];
    editor.addEventListener('wf-steps-change', () => this.#syncCounters());
    this.#syncCounters();
    this.#loadAgents();

    this.querySelector('#draft-btn').addEventListener('click', () => this.#draft());
    this.querySelector('#save-btn').addEventListener('click', () => this.#save({ run: false }));
    this.querySelector('#save-run-btn').addEventListener('click', () => this.#save({ run: true }));
  }

  async #loadAgents() {
    try {
      const res = await apiFetch('/agents?limit=100');
      if (!res.ok) return;
      const body = await res.json();
      const agents = (Array.isArray(body) ? body : body.data || [])
        .map((a) => ({ id: a.id, name: a.display_name || a.name || a.id }));
      this.querySelector('#editor').agents = agents;
    } catch { /* picker keeps only "Auto-select" — save still works */ }
  }

  #syncCounters() {
    const steps = this.querySelector('#editor').steps;
    const n = steps.length;
    this.querySelector('#step-count').textContent = n === 1 ? '1 step' : `${n} steps`;
    const unassigned = steps.filter((s) => !s.agentId).length;
    this.querySelector('#footnote').textContent = n === 0
      ? ''
      : unassigned === 0
        ? 'Every step has an agent'
        : `${unassigned} of ${n} step${n === 1 ? '' : 's'} auto-select an agent at run time`;
  }

  #notice(html) {
    const el = this.querySelector('#gen-notice');
    el.hidden = !html;
    el.innerHTML = html || '';
  }

  #setDrafting(on) {
    this.#drafting = on;
    this.querySelector('#drafting').hidden = !on;
    this.querySelector('#editor').style.display = on ? 'none' : '';
    const btn = this.querySelector('#draft-btn');
    btn.disabled = on;
    btn.innerHTML = on
      ? `<span class="spin">${icons.loader('', 13)}</span> Drafting…`
      : `${icons.sparkles('', 13)} Draft steps`;
  }

  async #draft() {
    if (this.#drafting) return;
    const desc = this.querySelector('#wf-desc').value.trim();
    if (!desc) {
      this.#notice('Describe what the workflow should do first — one or two sentences is enough.');
      this.querySelector('#wf-desc').focus();
      return;
    }
    this.#notice('');
    this.querySelector('#drafted-note').hidden = true;
    this.#setDrafting(true);
    try {
      const plan = await window.generateWorkflow(desc);
      const nameInput = this.querySelector('#wf-name');
      if (!nameInput.value.trim() && plan.name) nameInput.value = plan.name;
      this.querySelector('#editor').steps = (plan.steps || []).map((s) => ({
        taskDescription: s.task_description,
        agentId: s.agent_id,
        agentName: s.agent_name,
        suggested: true,
      }));
      this.querySelector('#drafted-note').hidden = false;
      this.#syncCounters();
    } catch (err) {
      this.#notice(this.#generateErrorHtml(err));
    } finally {
      this.#setDrafting(false);
    }
  }

  #generateErrorHtml(err) {
    if (err.status === 503) {
      return `AI drafting isn't available — this server has no OpenAI API key configured.
        You can still add steps manually below.`;
    }
    if (err.status === 400) {
      return `You don't have any agents yet, so there's nothing to plan with.
        <a href="/agents.html">Deploy an agent</a> first, then draft steps.`;
    }
    if (err.status === 422) {
      return `Nasiko couldn't draft steps from that description — try rephrasing it,
        or add the steps manually below.`;
    }
    return `Drafting failed: ${this.#esc(err.message)}`;
  }

  async #save({ run }) {
    const btn = this.querySelector(run ? '#save-run-btn' : '#save-btn');
    const steps = this.querySelector('#editor').steps
      .map((s) => ({ task_description: s.taskDescription.trim(), agent_id: s.agentId || undefined }))
      .filter((s) => s.task_description);
    if (!steps.length) {
      this.#notice('Add at least one step with instructions before saving.');
      return;
    }
    this.#notice('');
    btn.setAttribute('loading', '');
    try {
      const name = this.querySelector('#wf-name').value.trim();
      const description = this.querySelector('#wf-desc').value.trim();
      const workflow = await window.createWorkflow({
        name: name || undefined,
        description: description || undefined,
        steps,
      });
      let target = `/workflow.html?id=${encodeURIComponent(workflow.id)}`;
      if (run) {
        try {
          const started = await window.runWorkflow(workflow.id);
          target += `&exec=${encodeURIComponent(started.execution_id)}`;
        } catch (runErr) {
          // The workflow IS saved, so still land on the review screen — but say
          // why it didn't start. Swallowing this silently made a failed run
          // indistinguishable from a successful one.
          target += `&run_error=${encodeURIComponent(runErr.message || 'unknown error')}`;
        }
      }
      window.location.href = target;
    } catch (err) {
      btn.removeAttribute('loading');
      showToast(`Save failed: ${err.message}`);
    }
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s ?? '';
    return d.innerHTML;
  }
}

customElements.define('workflow-new-page', WorkflowNewPage);
