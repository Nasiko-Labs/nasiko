import { apiFetch } from '/common/services/api.js';
import '/common/components/app-module-nav.js';
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';

import styles from './departments-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class DepartmentsPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <app-module-nav module="org"></app-module-nav>
      <div class="page-head">
        <h1 class="title-page">Departments</h1>
        <span class="count-chips" id="dept-stats"></span>
        <div class="head-actions">
          <app-button variant="primary" size="sm" id="btn-create">${icons.plus('', 14)} Create department</app-button>
        </div>
      </div>
      <div class="warning-banner" id="warning-banner" hidden>
        ${icons.info('', 18)}
        <span id="warning-text"></span>
      </div>
      <smart-table id="dept-table" data-fn="fetchDepartments" search search-placeholder="Search departments..." limit="20"></smart-table>

      <app-modal id="create-modal" heading="Create Department">
        <div class="modal-form">
          <div class="field">
            <label>Name</label>
            <input type="text" id="dept-name" placeholder="Engineering" required />
          </div>
          <div class="field">
            <label>Description (optional)</label>
            <textarea id="dept-description" placeholder="Brief description of this department"></textarea>
          </div>
          <div class="form-actions">
            <app-button variant="secondary" size="sm" id="btn-cancel">Cancel</app-button>
            <app-button variant="primary" size="sm" id="btn-save">Create</app-button>
          </div>
        </div>
      </app-modal>
    `;

    const table = this.querySelector('#dept-table');
    const warnCell = (label) =>
      `<span class="cell-warn">${icons.info('', 14)} ${label}</span>`;
    const deptStatus = (row) => {
      const teams = row.teams_count ?? 0;
      if (!teams) return '<span class="badge badge--warning">Needs setup</span>';
      if (!row.manager) return '<span class="badge badge--error">Needs attention</span>';
      return '<span class="badge badge--success">Fully configured</span>';
    };
    table.columns = [
      { key: 'name', label: 'Name', width: '22%', render: (v) => `<span class="name-cell">${this.#esc(v)}</span>` },
      { key: 'manager', label: 'Manager', width: '20%', render: (v) =>
        v ? this.#esc(v) : warnCell('No manager assigned') },
      { key: 'teams_count', label: 'Teams', width: '14%', render: (v) =>
        (v ?? 0) > 0
          ? `<span class="tag-chip">${v} team${v === 1 ? '' : 's'}</span>`
          : warnCell('No teams') },
      { key: 'members_count', label: 'Members', width: '13%', render: (v) => String(v ?? 0) },
      { key: 'agents_count', label: 'Agents', width: '13%', render: (v) => String(v ?? 0) },
      { key: 'status', label: 'Status', width: '18%', render: (_, row) => deptStatus(row) },
    ];

    const modal = this.querySelector('#create-modal');
    this.querySelector('#btn-create').addEventListener('click', () => modal.open());
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());

    const saveBtn = this.querySelector('#btn-save');
    saveBtn.addEventListener('click', async () => {
      saveBtn.setAttribute('loading', '');
      try {
        const name = this.querySelector('#dept-name').value.trim();
        const description = this.querySelector('#dept-description').value.trim();

        if (!name) {
          showToast('Department name is required');
          return;
        }

        const body = { name };
        if (description) body.description = description;

        const res = await apiFetch('/departments', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });

        if (!res.ok) {
          const text = await res.text();
          throw new Error(text || 'Failed to create department');
        }

        modal.close();
        this.querySelector('#dept-name').value = '';
        this.querySelector('#dept-description').value = '';
        table.refresh();
        this.#loadStats();
        showToast('Department created');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        saveBtn.removeAttribute('loading');
      }
    });

    this.#loadStats();
  }

  async #loadStats() {
    try {
      const stats = await window.fetchDepartmentStats();
      const banner = this.querySelector('#warning-banner');
      const warningText = this.querySelector('#warning-text');
      const statsEl = this.querySelector('#dept-stats');

      if (stats.without_manager > 0) {
        warningText.textContent = `${stats.without_manager} department${stats.without_manager > 1 ? 's' : ''} without a manager assigned.`;
        banner.hidden = false;
      } else {
        banner.hidden = true;
      }

      statsEl.innerHTML = `<span class="count-chip"><b>${stats.total}</b> department${stats.total !== 1 ? 's' : ''}</span>`
        + `<span class="count-chip"><b>${stats.without_manager}</b> without manager</span>`;
    } catch {
      // Stats are non-critical; table still works without them
    }
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('departments-page', DepartmentsPage);
