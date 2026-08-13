import { apiFetch } from '/common/services/api.js';
import { authService } from '/common/services/auth-service.js';
import '/common/components/app-module-nav.js';
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { confirmDialog } from '/common/utils/confirm-dialog.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import '/common/components/smart-table.js';
import '/common/components/user-picker.js';

import styles from './teams-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class TeamsPage extends HTMLElement {
  #initialized = false;
  // Null while creating; the team's id while editing.
  #editingId = null;
  // Mutating a team is superuser-only server-side.
  #canEdit = false;

  async connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    // Resolved before the table renders so the actions column is correct on the
    // first paint rather than appearing a beat later.
    await authService.fetchCurrentUser().catch(() => null);
    this.#canEdit = authService.isSuperuser();

    this.innerHTML = `
      <app-module-nav module="org"></app-module-nav>
      <div class="page-head">
        <h1 class="title-page">Teams</h1>
        <span class="count-chips" id="teams-stats"><span class="pre-chip" style="--pre-chip-w:74px"></span><span class="pre-chip" style="--pre-chip-w:122px"></span></span>
        <div class="head-actions">
          <app-button variant="primary" size="sm" id="btn-create-team">${icons.plus('', 16)} Create team</app-button>
        </div>
      </div>
      <smart-table id="teams-table" data-fn="fetchTeams" search search-placeholder="Search teams..." limit="20"></smart-table>

      <app-modal id="create-team-modal" heading="Create Team">
        <div class="modal-form">
          <div class="field">
            <label>Name</label>
            <input type="text" id="team-name" placeholder="Engineering" required />
          </div>
          <div class="field">
            <label>Department</label>
            <select id="team-department">
              <option value="" disabled selected>Select department…</option>
            </select>
          </div>
          <div class="field">
            <label>Description (optional)</label>
            <textarea id="team-description" placeholder="Brief description of this team"></textarea>
          </div>
          <div class="field">
            <user-picker id="team-lead" label="Team lead (optional)"></user-picker>
          </div>
          <div class="form-error" id="team-error" hidden></div>
          <div class="form-actions">
            <app-button variant="secondary" size="sm" id="btn-cancel">Cancel</app-button>
            <app-button variant="primary" size="sm" id="btn-save">Create</app-button>
          </div>
        </div>
      </app-modal>
    `;

    this.#loadStats();
    this.#setupTable();
    this.#setupModal();
  }

  async #loadStats() {
    const el = this.querySelector('#teams-stats');
    try {
      const stats = await window.fetchTeamStats();
      if (!stats) return;
      el.innerHTML = `<span class="count-chip"><b>${stats.total}</b> team${stats.total !== 1 ? 's' : ''}</span>`
        + `<span class="count-chip"><b>${stats.empty_teams}</b> without members</span>`;
    } catch {
      el.innerHTML = '';
    }
  }

  #setupTable() {
    const warnCell = (label) =>
      `<span class="cell-warn">${icons.info('', 14)} ${label}</span>`;
    const table = this.querySelector('#teams-table');
    table.columns = [
      { key: 'name', label: 'Name', width: '22%', render: (v) => `<strong>${this.#esc(v)}</strong>` },
      // `department_name`, not `department` — TeamView (ee/server/src/teams.rs)
      // has no `department` field, so this column always read "--".
      { key: 'department_name', label: 'Department', width: '20%', render: (v) =>
        v ? `<span class="tag-chip">${this.#esc(v)}</span>` : '<span class="muted">--</span>' },
      { key: 'team_lead', label: 'Team lead', width: '18%', render: (v) =>
        v?.name ? this.#esc(v.name) : warnCell('No lead assigned') },
      { key: 'members_count', label: 'Members', width: '14%', render: (v) =>
        (v ?? 0) > 0 ? String(v) : warnCell('No members') },
      { key: 'agents_count', label: 'Agents', width: '12%', render: (v) => v != null ? String(v) : '0' },
      // PUT/DELETE /teams/{id} are superuser-only server-side.
      { key: 'actions', label: '', width: '14%', render: (_, row) =>
        !this.#canEdit ? '' : `
        <span class="row-actions">
          <button type="button" class="row-btn" data-edit="${this.#escAttr(row.id)}">Edit</button>
          <button type="button" class="row-btn row-btn--danger" data-delete="${this.#escAttr(row.id)}"
            data-name="${this.#escAttr(row.name)}">Delete</button>
        </span>` },
    ];

    // Delegated — smart-table re-renders rows on every search/page change.
    table.addEventListener('click', (e) => {
      const edit = e.target.closest('[data-edit]');
      if (edit) {
        this.#openEdit(edit.dataset.edit);
        return;
      }
      const del = e.target.closest('[data-delete]');
      if (del) this.#deleteTeam(del.dataset.delete, del.dataset.name);
    });
  }

  #setupModal() {
    const modal = this.querySelector('#create-team-modal');

    this.querySelector('#btn-create-team').addEventListener('click', () => this.#openCreate());
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());
    this.querySelector('#btn-save').addEventListener('click', () => this.#submit());
  }

  async #openCreate() {
    this.#editingId = null;
    const modal = this.querySelector('#create-team-modal');
    modal.setAttribute('heading', 'Create Team');
    this.querySelector('#btn-save').label = 'Create';
    this.querySelector('#team-name').value = '';
    this.querySelector('#team-description').value = '';
    this.querySelector('#team-error').hidden = true;
    this.querySelector('#team-lead').reset();
    await this.#loadDepartments();
    const select = this.querySelector('#team-department');
    select.disabled = false;
    select.value = '';
    modal.open();
  }

  async #openEdit(id) {
    const modal = this.querySelector('#create-team-modal');
    const picker = this.querySelector('#team-lead');
    this.#editingId = id;
    modal.setAttribute('heading', 'Edit Team');
    this.querySelector('#btn-save').label = 'Save changes';
    this.querySelector('#team-error').hidden = true;
    picker.reset();
    await this.#loadDepartments();
    modal.open();

    try {
      const res = await apiFetch(`/teams/${encodeURIComponent(id)}`);
      if (!res.ok) throw new Error((await res.text()) || res.statusText);
      const team = await res.json();
      this.querySelector('#team-name').value = team.name || '';
      this.querySelector('#team-description').value = team.description || '';
      // PUT /teams/{id} cannot move a team between departments, so show the
      // current one read-only rather than offering an edit that won't apply.
      const select = this.querySelector('#team-department');
      select.value = team.department_id || '';
      select.disabled = true;
      picker.value = team.team_lead?.id ? team.team_lead : null;
    } catch (e) {
      this.#showError(`Could not load team: ${e.message}`);
    }
  }

  async #submit() {
    const saveBtn = this.querySelector('#btn-save');
    const name = this.querySelector('#team-name').value.trim();
    const description = this.querySelector('#team-description').value.trim();
    const departmentId = this.querySelector('#team-department').value;
    const editing = Boolean(this.#editingId);

    if (!name) {
      this.#showError('Team name is required');
      return;
    }
    // POST /teams requires department_id (server 422s on null) —
    // every team belongs to a department by design.
    if (!editing && !departmentId) {
      this.#showError('Department is required');
      return;
    }

    // Description uses `COALESCE($n, description)` server-side, so send the
    // empty string (not null) when the user clears it.
    const body = { name, description };
    if (!editing) body.department_id = departmentId;
    const lead = this.querySelector('#team-lead').value;
    if (lead) body.lead_id = lead.id;
    else if (editing) body.clear_lead = true;

    saveBtn.setAttribute('loading', '');
    try {
      const path = editing ? `/teams/${encodeURIComponent(this.#editingId)}` : '/teams';
      const res = await apiFetch(path, {
        method: editing ? 'PUT' : 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error((await res.text()) || 'Request failed');

      this.querySelector('#create-team-modal').close();
      this.querySelector('#teams-table').refresh();
      this.#loadStats();
      showToast(editing ? 'Team updated' : 'Team created');
    } catch (e) {
      this.#showError(e.message);
    } finally {
      saveBtn.removeAttribute('loading');
    }
  }

  async #deleteTeam(id, name) {
    const confirmed = await confirmDialog({
      title: `Delete ${name}`,
      message: 'Its members will be unassigned, not deleted. This cannot be undone.',
      confirmLabel: 'Delete',
      danger: true,
    });
    if (!confirmed) return;
    try {
      const res = await apiFetch(`/teams/${encodeURIComponent(id)}`, { method: 'DELETE' });
      if (!res.ok) throw new Error((await res.text()) || 'Failed to delete team');
      this.querySelector('#teams-table').refresh();
      this.#loadStats();
      showToast('Team deleted');
    } catch (e) {
      showToast(`Failed: ${e.message}`);
    }
  }

  #showError(message) {
    const el = this.querySelector('#team-error');
    el.textContent = message;
    el.hidden = false;
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s ?? '';
    return d.innerHTML;
  }

  #escAttr(s) {
    return String(s ?? '').replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }

  async #loadDepartments() {
    const select = this.querySelector('#team-department');
    try {
      const departments = await window.fetchDepartmentList();
      const current = select.value;
      select.innerHTML = '<option value="" disabled selected>Select department…</option>';
      if (departments && departments.length) {
        for (const dept of departments) {
          const opt = document.createElement('option');
          opt.value = dept.id;
          opt.textContent = dept.name;
          select.appendChild(opt);
        }
      }
      select.value = current;
    } catch {
      // keep existing options if fetch fails
    }
  }
}

customElements.define('teams-page', TeamsPage);
