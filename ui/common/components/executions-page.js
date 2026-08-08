/**
 * All executions — workflow runs across every workflow (GET /api/maf/executions).
 *
 * Active tab shows in-flight runs with a live step timeline (the list rows
 * carry snapshotted step_results; the page re-polls the list every 1.5s
 * while anything is pending/running — there is no run SSE). History tab
 * lists finished runs, collapsed, with a status filter.
 *
 * @element executions-page
 */
import { icons } from '/common/utils/icons.js';
import { timeAgo, formatDisplay } from '/common/utils/date-utils.js';
import { fmtDuration, fmtTokens } from '/common/utils/units.js';
import { attachSlidingIndicator } from '/common/utils/tab-indicator.js';
import '/common/components/app-module-nav.js';
import '/common/components/wf-run-steps.js';

import styles from './executions-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const POLL_MS = 1500;
const ACTIVE = new Set(['pending', 'running']);
const STATUS_BADGES = { success: 'badge--success', failed: 'badge--error', running: 'badge--warning', pending: 'badge--neutral' };

class ExecutionsPage extends HTMLElement {
  #initialized = false;
  #executions = [];
  #tab = 'active';
  #statusFilter = 'all';
  #expanded = new Set();
  #pollTimer = null;
  #loaded = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <app-module-nav module="orchestrator"></app-module-nav>
      <h1 class="title-page page-title">All executions</h1>
      <div class="tabs" role="tablist">
        <button type="button" class="tab is-active" role="tab" data-tab="active" aria-selected="true">Active</button>
        <button type="button" class="tab" role="tab" data-tab="history" aria-selected="false">History</button>
      </div>
      <div class="list-area" id="list-area">${this.#skeleton()}</div>
    `;

    attachSlidingIndicator(this.querySelector('.tabs'), '.tab', '.is-active');
    this.querySelector('.tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('[data-tab]');
      if (!tab || tab.dataset.tab === this.#tab) return;
      this.#tab = tab.dataset.tab;
      this.querySelectorAll('.tab').forEach((t) => {
        const active = t.dataset.tab === this.#tab;
        t.classList.toggle('is-active', active);
        t.setAttribute('aria-selected', String(active));
      });
      this.#renderList();
    });

    const area = this.querySelector('#list-area');
    area.addEventListener('click', (e) => {
      const filterBtn = e.target.closest('[data-filter]');
      if (filterBtn) {
        this.#statusFilter = filterBtn.dataset.filter;
        this.#renderHistoryRuns(); // seg-ctrl stays mounted so its indicator slides
        return;
      }
      const toggle = e.target.closest('[data-toggle]');
      if (toggle) {
        const id = toggle.dataset.toggle;
        this.#expanded.has(id) ? this.#expanded.delete(id) : this.#expanded.add(id);
        this.#tab === 'history' ? this.#renderHistoryRuns() : this.#renderList();
      }
    });

    this.#load();
  }

  disconnectedCallback() {
    clearTimeout(this.#pollTimer);
    this.#pollTimer = null;
  }

  async #load() {
    try {
      this.#executions = await window.fetchAllExecutions();
      this.#loaded = true;
      this.#renderList();
      this.#pollIfActive();
    } catch (err) {
      this.querySelector('#list-area').innerHTML =
        `<p class="load-error">Failed to load executions: ${this.#esc(err.message)}</p>`;
    }
  }

  #pollIfActive() {
    if (!this.#executions.some((e) => ACTIVE.has(e.status))) return;
    this.#pollTimer = setTimeout(async () => {
      try {
        this.#executions = await window.fetchAllExecutions();
        if (this.#tab === 'active') this.#refreshActive();
      } catch { /* transient poll failure — keep trying */ }
      this.#pollIfActive();
    }, POLL_MS);
  }

  /** In-place update of open active cards; full re-render only when the
   *  active set changes (keeps per-step tab state stable while polling). */
  #refreshActive() {
    const active = this.#executions.filter((e) => ACTIVE.has(e.status));
    const rendered = [...this.querySelectorAll('.run-card[data-card]')].map((c) => c.dataset.card);
    const sameSet = active.length === rendered.length && active.every((e) => rendered.includes(e.id));
    if (!sameSet) {
      this.#renderList();
      return;
    }
    for (const exec of active) {
      const card = this.querySelector(`.run-card[data-card="${CSS.escape(exec.id)}"]`);
      const metaEl = card?.querySelector('.run-card-meta');
      if (metaEl) metaEl.innerHTML = this.#metaHtml(exec);
    }
    this.#hydrateSteps(active);
  }

  #renderList() {
    const area = this.querySelector('#list-area');
    if (!this.#loaded) return;

    if (this.#tab === 'active') {
      const active = this.#executions.filter((e) => ACTIVE.has(e.status));
      if (!this.#executions.length) {
        area.innerHTML = this.#emptyState({
          icon: icons.workflow('', 32),
          title: 'No workflow runs yet',
          sub: 'Create your first workflow by chaining agents together.',
          action: `<a class="cta-btn" href="/workflow-new.html">Create workflow ${icons.plus('', 13)}</a>`,
        });
        return;
      }
      if (!active.length) {
        area.innerHTML = this.#emptyState({
          icon: icons.play('', 32),
          title: 'Your active runs will appear here',
          sub: 'Monitor live workflow executions, track progress across each step, and inspect outputs as they are generated.',
          action: `<a class="cta-btn is-secondary" href="/workflows.html">Browse workflows</a>`,
        });
        return;
      }
      area.innerHTML = `<div class="run-list">${active.map((e) => this.#runCard(e, { open: true })).join('')}</div>`;
      this.#hydrateSteps(active);
      return;
    }

    // History tab
    const finished = this.#executions.filter((e) => !ACTIVE.has(e.status));
    if (!finished.length) {
      area.innerHTML = this.#emptyState({
        icon: icons.workflow('', 32),
        title: 'No finished runs yet',
        sub: 'Completed and failed workflow runs land here with their full step timelines.',
        action: `<a class="cta-btn is-secondary" href="/workflows.html">Browse workflows</a>`,
      });
      return;
    }
    area.innerHTML = `
      <fieldset class="seg-ctrl">
        <legend>Status</legend>
        ${[['all', 'All'], ['success', 'Completed'], ['failed', 'Failed']].map(([key, label]) => `
          <label><input type="radio" name="status-filter" data-filter="${key}"
            ${this.#statusFilter === key ? 'checked' : ''}>${label}</label>`).join('')}
      </fieldset>
      <div class="run-list"></div>`;
    attachSlidingIndicator(area.querySelector('.seg-ctrl'), 'label', ':has(input:checked)', { pill: true });
    this.#renderHistoryRuns();
  }

  /** Fills `.run-list` only — the history seg-ctrl stays mounted so filter
   *  switches animate its indicator instead of rebuilding the control. */
  #renderHistoryRuns() {
    const list = this.querySelector('.run-list');
    if (!list) return;
    const finished = this.#executions.filter((e) => !ACTIVE.has(e.status));
    const filtered = this.#statusFilter === 'all'
      ? finished
      : finished.filter((e) => e.status === this.#statusFilter);
    list.innerHTML = filtered.length
      ? filtered.map((e) => this.#runCard(e, { open: this.#expanded.has(e.id) })).join('')
      : '<p class="filter-empty">No runs match this filter.</p>';
    this.#hydrateSteps(filtered.filter((e) => this.#expanded.has(e.id)));
  }

  /** wf-run-steps takes data via property — assign after the HTML lands. */
  #hydrateSteps(rows) {
    for (const exec of rows) {
      const el = this.querySelector(`wf-run-steps[data-exec="${CSS.escape(exec.id)}"]`);
      if (el) el.steps = exec.step_results || [];
    }
  }

  #metaHtml(exec) {
    const stepCount = exec.step_results?.length;
    const meta = [
      stepCount ? (stepCount === 1 ? '1 step' : `${stepCount} steps`) : '',
      exec.created_at ? (ACTIVE.has(exec.status) ? `Started ${timeAgo(exec.created_at)}` : formatDisplay(new Date(exec.created_at))) : '',
      exec.duration_ms != null ? fmtDuration(exec.duration_ms) : '',
      fmtTokens(exec.tokens_used),
    ].filter(Boolean);
    const statusCls = STATUS_BADGES[exec.status] || 'badge--neutral';
    return meta.map((m) => `<span class="badge badge--muted">${this.#esc(m)}</span>`).join('') +
      `<span class="badge ${statusCls}"><span class="badge__dot"></span>${this.#esc(exec.status)}</span>`;
  }

  #runCard(exec, { open }) {
    const orphaned = !exec.workflow_name || exec.workflow_status === 'deleted';
    const title = `${exec.workflow_name || 'Deleted workflow'} #${exec.execution_number}`;
    return `
      <div class="run-card" data-card="${this.#esc(exec.id)}">
        <div class="run-card-head">
          <span class="run-title">${this.#esc(title)}</span>
          ${orphaned ? `<span class="badge badge--error">${icons.info('', 12)} Workflow not found</span>` : ''}
          <span class="head-spacer"></span>
          ${!orphaned && exec.maf_id ? `<a class="open-wf" href="/workflow.html?id=${encodeURIComponent(exec.maf_id)}&exec=${encodeURIComponent(exec.id)}">Open workflow</a>` : ''}
          <button type="button" class="toggle-btn" data-toggle="${this.#esc(exec.id)}"
            aria-expanded="${open}" aria-label="${open ? 'Collapse' : 'Expand'} run">
            ${open ? icons.chevronUp('', 16) : icons.chevronDown('', 16)}
          </button>
        </div>
        <div class="run-card-meta">${this.#metaHtml(exec)}</div>
        ${open ? `<wf-run-steps surface="sand" data-exec="${this.#esc(exec.id)}"></wf-run-steps>` : ''}
        ${open && exec.error ? `<div class="run-error">${this.#esc(exec.error)}</div>` : ''}
      </div>`;
  }

  #emptyState({ icon, title, sub, action }) {
    return `
      <div class="runs-empty">
        <span class="empty-tile">${icon}</span>
        <h2 class="empty-title">${title}</h2>
        <p class="empty-sub">${sub}</p>
        ${action}
      </div>`;
  }

  #skeleton() {
    return `<div class="run-list">${Array.from({ length: 2 }, () => `
      <div class="run-card is-skeleton">
        <div class="skel-line skel-line--name"></div>
        <div class="skel-tags"><div class="skel-tag"></div><div class="skel-tag"></div><div class="skel-tag"></div></div>
        <div class="skel-line skel-line--desc1"></div>
      </div>`).join('')}</div>`;
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s ?? '';
    return d.innerHTML;
  }
}

customElements.define('executions-page', ExecutionsPage);
