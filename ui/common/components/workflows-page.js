/**
 * Workflows library — reusable multi-agent sequences (MAF workflows).
 *
 * Data: GET /api/maf/workflows via window.fetchWorkflows; last-run status is
 * joined client-side from GET /api/maf/executions (the list API carries no
 * last-run info, only execution_count).
 *
 * @element workflows-page
 */
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { confirmDialog } from '/common/utils/confirm-dialog.js';
import { timeAgo, formatDisplay } from '/common/utils/date-utils.js';
import '/common/components/app-module-nav.js';
import '/common/components/app-action-menu.js';

import styles from './workflows-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const MENU_ITEMS = JSON.stringify([
  { id: 'open', label: 'Open workflow' },
  { id: 'run', label: 'Run now' },
  { id: 'delete', label: 'Delete workflow' },
]);

class WorkflowsPage extends HTMLElement {
  #initialized = false;
  #workflows = [];
  #lastRun = new Map(); // maf_id → latest execution row

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <app-module-nav module="orchestrator"></app-module-nav>
      <header class="page-head">
        <div class="page-head-text">
          <h1 class="title-page">Workflows</h1>
          <p class="page-sub">Reusable multi-agent sequences you can run on demand.</p>
        </div>
        <a class="new-wf-btn" href="/workflow-new.html">Create workflow ${icons.plus('', 13)}</a>
      </header>
      <div class="grid" id="wf-grid">${this.#skeletonCards()}</div>
    `;

    const grid = this.querySelector('#wf-grid');
    grid.addEventListener('click', (e) => {
      if (e.target.closest('app-action-menu') || e.target.closest('a')) return;
      const card = e.target.closest('.wf-card[data-id]');
      if (card) window.location.href = `/workflow.html?id=${encodeURIComponent(card.dataset.id)}`;
    });
    grid.addEventListener('action-select', (e) => {
      const card = e.target.closest('.wf-card[data-id]');
      if (card) this.#onAction(e.detail.id, card.dataset.id);
    });

    this.#load();
  }

  async #load() {
    try {
      const [workflows, executions] = await Promise.all([
        window.fetchWorkflows(),
        window.fetchAllExecutions().catch(() => []),
      ]);
      this.#workflows = workflows;
      // Executions come newest-first; keep the first row seen per workflow.
      this.#lastRun = new Map();
      for (const exec of executions) {
        if (!this.#lastRun.has(exec.maf_id)) this.#lastRun.set(exec.maf_id, exec);
      }
      this.#renderGrid();
    } catch (err) {
      this.querySelector('#wf-grid').innerHTML =
        `<p class="load-error">Failed to load workflows: ${this.#esc(err.message)}</p>`;
    }
  }

  async #onAction(action, id) {
    if (action === 'open') {
      window.location.href = `/workflow.html?id=${encodeURIComponent(id)}`;
    } else if (action === 'run') {
      try {
        const run = await window.runWorkflow(id);
        window.location.href = `/workflow.html?id=${encodeURIComponent(id)}&exec=${encodeURIComponent(run.execution_id)}`;
      } catch (err) {
        showToast(`Run failed: ${err.message}`);
      }
    } else if (action === 'delete') {
      const wf = this.#workflows.find((w) => w.id === id);
      const confirmed = await confirmDialog({
        title: `Delete ${wf?.name || 'workflow'}`,
        message: 'Its execution history goes with it. This cannot be undone.',
        confirmLabel: 'Delete',
        danger: true,
      });
      if (!confirmed) return;
      try {
        await window.deleteWorkflow(id);
        this.#workflows = this.#workflows.filter((w) => w.id !== id);
        this.#renderGrid();
        showToast('Workflow deleted');
      } catch (err) {
        showToast(`Delete failed: ${err.message}`);
      }
    }
  }

  #statusLine(wf) {
    const last = this.#lastRun.get(wf.id);
    if (!last || wf.execution_count === 0) {
      return { cls: 'is-idle', text: 'Not run yet' };
    }
    const when = timeAgo(last.completed_at || last.created_at);
    if (last.status === 'success') return { cls: 'is-success', text: `Last run succeeded ${when}` };
    if (last.status === 'failed') return { cls: 'is-failed', text: `Last run failed ${when}` };
    return { cls: 'is-running', text: 'Running now' };
  }

  #card(wf) {
    const steps = wf.maf_json?.steps || [];
    const agents = [...new Set(steps.map((s) => s.agent_name).filter(Boolean))];
    const description = wf.description || wf.maf_json?.description || '';
    const status = this.#statusLine(wf);
    const runs = wf.execution_count === 1 ? '1 run' : `${wf.execution_count} runs`;
    return `
      <div class="wf-card" data-id="${this.#esc(wf.id)}" role="link" tabindex="0"
        aria-label="Open ${this.#esc(wf.name)}">
        <div class="wf-card-top">
          <span class="wf-name">${this.#esc(wf.name)}</span>
          <app-action-menu trigger-title="Workflow actions" items='${MENU_ITEMS}'>
            ${icons.moreVertical('', 16)}
          </app-action-menu>
        </div>
        <div class="wf-badges">
          <span class="badge badge--muted">${steps.length === 1 ? '1 step' : `${steps.length} steps`}</span>
          <span class="badge badge--muted">${this.#esc(runs)}</span>
          ${wf.created_at ? `<span class="badge badge--muted">Created ${this.#esc(formatDisplay(new Date(wf.created_at)))}</span>` : ''}
        </div>
        ${description ? `<p class="wf-desc">${this.#esc(description)}</p>` : ''}
        ${agents.length ? `<p class="wf-agents">${this.#esc(agents.join(' · '))}</p>` : ''}
        <div class="wf-status ${status.cls}">
          <span class="wf-dot"></span>
          <span class="wf-status-text">${this.#esc(status.text)}</span>
        </div>
      </div>`;
  }

  #renderGrid() {
    const grid = this.querySelector('#wf-grid');
    if (!this.#workflows.length) {
      grid.className = 'empty-wrap';
      grid.innerHTML = `
        <div class="wf-empty">
          <span class="empty-tile">${icons.workflow('', 32)}</span>
          <h2 class="empty-title">No workflows yet</h2>
          <p class="empty-sub">Chain agents into a repeatable sequence. Describe what you want to
            automate and Nasiko drafts the steps for you.</p>
          <div class="empty-pills">
            <span class="process-pill">${icons.editThin('', 12)} Describe</span>
            ${icons.chevronRight('empty-arrow', 12)}
            <span class="process-pill">${icons.checkCircle('', 12)} Review steps</span>
            ${icons.chevronRight('empty-arrow', 12)}
            <span class="process-pill">${icons.play('', 12)} Run</span>
          </div>
          <a class="new-wf-btn is-lg" href="/workflow-new.html">Create workflow ${icons.plus('', 13)}</a>
        </div>`;
      return;
    }
    grid.className = 'grid';
    grid.innerHTML = this.#workflows.map((wf) => this.#card(wf)).join('');
  }

  #skeletonCards() {
    return Array.from({ length: 3 }, () => `
      <div class="wf-card is-skeleton">
        <div class="skel-line skel-line--name"></div>
        <div class="skel-tags"><div class="skel-tag"></div><div class="skel-tag"></div></div>
        <div class="skel-line skel-line--desc1"></div>
        <div class="skel-line skel-line--desc2"></div>
      </div>`).join('');
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s ?? '';
    return d.innerHTML;
  }
}

customElements.define('workflows-page', WorkflowsPage);
