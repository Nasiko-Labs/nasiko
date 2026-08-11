import { apiFetch } from '/common/services/api.js';
import { authService } from '/common/services/auth-service.js';
import '/common/components/app-module-nav.js';
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import '/common/components/user-picker.js';

import styles from './departments-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class DepartmentsPage extends HTMLElement {
  #initialized = false;
  // Null while creating; the department's id while editing.
  #editingId = null;
  // Mutating a department is superuser-only server-side.
  #canEdit = false;

  async connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    // Resolved before the table renders so the actions column is right on the
    // first paint rather than appearing a beat later.
    await authService.fetchCurrentUser().catch(() => null);
    this.#canEdit = authService.isSuperuser();

    this.innerHTML = `
      <app-module-nav module="org"></app-module-nav>
      <div class="page-head">
        <h1 class="title-page">Departments</h1>
        <span class="count-chips" id="dept-stats"><span class="pre-chip" style="--pre-chip-w:106px"></span><span class="pre-chip" style="--pre-chip-w:124px"></span></span>
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
          <div class="field">
            <user-picker id="dept-manager" label="Manager (optional)"></user-picker>
          </div>
          <div class="form-error" id="dept-error" hidden></div>
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
    // `DepartmentView` exposes a `teams` array, not a `teams_count` — reading
    // the latter made every row count as zero teams, so every department showed
    // "Needs setup" regardless of its actual state.
    const teamCount = (row) => row.teams_count ?? row.teams?.length ?? 0;
    const deptStatus = (row) => {
      if (!teamCount(row)) return '<span class="badge badge--warning">Needs setup</span>';
      if (!row.manager) return '<span class="badge badge--error">Needs attention</span>';
      return '<span class="badge badge--success">Fully configured</span>';
    };
    table.columns = [
      { key: 'name', label: 'Name', width: '20%', render: (v) => `<span class="name-cell">${this.#esc(v)}</span>` },
      // `manager` is a {id, name} object (DepartmentView.manager), not a string —
      // passing it straight to #esc rendered a literal "[object Object]".
      { key: 'manager', label: 'Manager', width: '18%', render: (v) =>
        v?.name ? this.#esc(v.name) : warnCell('No manager assigned') },
      { key: 'teams_count', label: 'Teams', width: '12%', render: (_, row) => {
        const n = teamCount(row);
        return n > 0 ? `<span class="tag-chip">${n} team${n === 1 ? '' : 's'}</span>` : warnCell('No teams');
      } },
      { key: 'members_count', label: 'Members', width: '11%', render: (v) => String(v ?? 0) },
      { key: 'agents_count', label: 'Agents', width: '11%', render: (v) => String(v ?? 0) },
      { key: 'status', label: 'Status', width: '16%', render: (_, row) => deptStatus(row) },
      // PUT/DELETE /departments/{id} are superuser-only on the server, so anyone
      // else would just get a 403 from a button that looked available.
      { key: 'actions', label: '', width: '12%', render: (_, row) =>
        !this.#canEdit ? '' : `
        <span class="row-actions">
          <button type="button" class="row-btn" data-edit="${this.#escAttr(row.id)}">Edit</button>
          <button type="button" class="row-btn row-btn--danger" data-delete="${this.#escAttr(row.id)}"
            data-name="${this.#escAttr(row.name)}">Delete</button>
        </span>` },
    ];

    const modal = this.querySelector('#create-modal');
    this.querySelector('#btn-create').addEventListener('click', () => this.#openCreate());
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());
    this.querySelector('#btn-save').addEventListener('click', () => this.#submit());

    // Row actions are delegated: smart-table re-renders its rows on every
    // search/page change, so per-button listeners would go stale.
    table.addEventListener('click', (e) => {
      const edit = e.target.closest('[data-edit]');
      if (edit) {
        this.#openEdit(edit.dataset.edit);
        return;
      }
      const del = e.target.closest('[data-delete]');
      if (del) this.#deleteDept(del.dataset.delete, del.dataset.name);
    });

    this.#loadStats();
  }

  /* ── Create / edit ─────────────────────────────────────────────────────── */

  #openCreate() {
    this.#editingId = null;
    const modal = this.querySelector('#create-modal');
    modal.setAttribute('heading', 'Create Department');
    this.querySelector('#btn-save').label = 'Create';
    this.querySelector('#dept-name').value = '';
    this.querySelector('#dept-description').value = '';
    this.querySelector('#dept-manager').reset();
    this.querySelector('#dept-error').hidden = true;
    modal.open();
  }

  async #openEdit(id) {
    const modal = this.querySelector('#create-modal');
    const picker = this.querySelector('#dept-manager');
    this.#editingId = id;
    modal.setAttribute('heading', 'Edit Department');
    this.querySelector('#btn-save').label = 'Save changes';
    this.querySelector('#dept-error').hidden = true;
    picker.reset();
    modal.open();

    try {
      const res = await apiFetch(`/departments/${encodeURIComponent(id)}`);
      if (!res.ok) throw new Error((await res.text()) || res.statusText);
      const dept = await res.json();
      this.querySelector('#dept-name').value = dept.name || '';
      this.querySelector('#dept-description').value = dept.description || '';
      picker.value = dept.manager?.id ? dept.manager : null;
    } catch (e) {
      this.#showError(`Could not load department: ${e.message}`);
    }
  }

  async #submit() {
    const saveBtn = this.querySelector('#btn-save');
    const name = this.querySelector('#dept-name').value.trim();
    const description = this.querySelector('#dept-description').value.trim();
    if (!name) {
      this.#showError('Department name is required');
      return;
    }

    const editing = Boolean(this.#editingId);
    // Always send the field, even empty: the server updates description with
    // `COALESCE($3, description)`, so a null would silently keep the old text
    // when the user meant to clear it. An empty string is the closest the
    // current contract gets to "no description".
    const body = { name, description };
    const manager = this.querySelector('#dept-manager').value;
    if (manager) body.manager_id = manager.id;
    // The server distinguishes "leave the manager alone" (field absent) from
    // "remove them" (clear_manager), so an edit that dropped the manager has to
    // say so explicitly.
    else if (editing) body.clear_manager = true;

    saveBtn.setAttribute('loading', '');
    try {
      const path = editing ? `/departments/${encodeURIComponent(this.#editingId)}` : '/departments';
      const res = await apiFetch(path, {
        method: editing ? 'PUT' : 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error((await res.text()) || 'Request failed');

      this.querySelector('#create-modal').close();
      this.querySelector('#dept-table').refresh();
      this.#loadStats();
      showToast(editing ? 'Department updated' : 'Department created');
    } catch (e) {
      this.#showError(e.message);
    } finally {
      saveBtn.removeAttribute('loading');
    }
  }

  async #deleteDept(id, name) {
    if (!confirm(`Delete department "${name}"? Its teams and members are unassigned, not deleted.`)) return;
    try {
      const res = await apiFetch(`/departments/${encodeURIComponent(id)}`, { method: 'DELETE' });
      if (!res.ok) throw new Error((await res.text()) || 'Failed to delete department');
      this.querySelector('#dept-table').refresh();
      this.#loadStats();
      showToast('Department deleted');
    } catch (e) {
      showToast(`Failed: ${e.message}`);
    }
  }

  #showError(message) {
    const el = this.querySelector('#dept-error');
    el.textContent = message;
    el.hidden = false;
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

  // #esc escapes text nodes, which leaves `"` intact — fine between tags, not
  // inside an attribute.
  #escAttr(s) {
    return String(s ?? '').replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('departments-page', DepartmentsPage);
