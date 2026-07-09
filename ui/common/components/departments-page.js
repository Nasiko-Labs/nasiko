import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import '/common/components/app-badge.js';

import styles from './departments-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class DepartmentsPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <div class="warning-banner" id="warning-banner" hidden>
        ${icons.info('', 18)}
        <span id="warning-text"></span>
      </div>
      <div class="page-header">
        <span></span>
        <app-button variant="primary" size="sm" id="btn-create">${icons.plus('', 14)} Create Department</app-button>
      </div>
      <div class="dept-stats" id="dept-stats"></div>
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
    table.columns = [
      { key: 'name', label: 'Name', width: '25%', render: (v) => `<span class="name-cell">${this.#esc(v)}</span>` },
      { key: 'manager', label: 'Manager', width: '20%', render: (v) => v ? this.#esc(v) : '<span class="muted">--</span>' },
      { key: 'teams_count', label: 'Teams', width: '12%', render: (v) => String(v ?? 0) },
      { key: 'members_count', label: 'Members', width: '12%', render: (v) => String(v ?? 0) },
      { key: 'agents_count', label: 'Agents', width: '12%', render: (v) => String(v ?? 0) },
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

      statsEl.innerHTML = `<span class="count">${stats.total}</span> department${stats.total !== 1 ? 's' : ''}, <span class="count">${stats.without_manager}</span> without manager`;
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
