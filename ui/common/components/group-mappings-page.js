import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import '/common/components/app-badge.js';

import styles from './group-mappings-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const ROLES = ['member', 'team_member', 'team_lead', 'department_manager', 'admin'];

class GroupMappingsPage extends HTMLElement {
  #initialized = false;
  #departments = [];
  #teams = [];

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#render();
    this.#bind();
  }

  #render() {
    const roleOptions = ['<option value="">-- None --</option>']
      .concat(ROLES.map((r) => `<option value="${r}">${r}</option>`))
      .join('');

    this.innerHTML = `
      <div class="page-header">
        <span></span>
        <app-button variant="secondary" size="sm" id="btn-sync-azure">${icons.refresh('', 14)} Sync Azure AD Directory</app-button>
        <app-button variant="dark" size="sm" id="btn-create">${icons.plus('', 14)} New Mapping</app-button>
      </div>
      <smart-table id="mappings-table" data-fn="fetchGroupMappings" limit="20"></smart-table>

      <app-modal id="mapping-modal" heading="New Group Mapping">
        <div class="modal-form">
          <div class="field">
            <label>Entra Security Group Object ID</label>
            <input type="text" id="mapping-group-id" placeholder="e.g. 11111111-1111-1111-1111-111111111111" required />
          </div>
          <div class="field">
            <label>Role</label>
            <select id="mapping-role">${roleOptions}</select>
          </div>
          <div class="field">
            <label>Department</label>
            <select id="mapping-department"><option value="">-- None --</option></select>
          </div>
          <div class="field">
            <label>Team</label>
            <select id="mapping-team"><option value="">-- None --</option></select>
          </div>
          <div class="field">
            <label>Description (optional)</label>
            <input type="text" id="mapping-description" placeholder="e.g. Platform team leads" />
          </div>
          <p class="hint">At least one of Role / Department / Team must be set.</p>
          <div class="form-actions" data-slot="footer">
            <app-button variant="secondary" id="btn-cancel">Cancel</app-button>
            <app-button variant="primary" id="btn-save">Save</app-button>
          </div>
        </div>
      </app-modal>
    `;

    const table = this.querySelector('#mappings-table');
    table.columns = [
      { key: 'external_group_id', label: 'Entra Group Object ID', width: '28%', render: (v) => `<code>${this.#esc(v)}</code>` },
      { key: 'role', label: 'Role', width: '14%', render: (v) => v ? `<app-badge variant="info">${this.#esc(v)}</app-badge>` : '<span class="muted">--</span>' },
      { key: 'department_id', label: 'Department', width: '16%', render: (v) => this.#deptName(v) },
      { key: 'team_id', label: 'Team', width: '16%', render: (v) => this.#teamName(v) },
      { key: 'description', label: 'Description', width: '16%', render: (v) => v ? this.#esc(v) : '<span class="muted">--</span>' },
      { key: 'actions', label: '', width: '10%', render: (_, row) => {
        const esc = (s) => (s == null ? '' : String(s)).replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        const edit = `<button class="action-btn" data-action="edit" data-id="${esc(row.id)}"` +
          ` data-external-group-id="${esc(row.external_group_id)}" data-role="${esc(row.role)}"` +
          ` data-department-id="${esc(row.department_id)}" data-team-id="${esc(row.team_id)}"` +
          ` data-description="${esc(row.description)}" title="Edit">${icons.edit('', 16)}</button>`;
        const del = `<button class="action-btn action-btn--danger" data-action="delete" data-id="${esc(row.id)}" data-group="${esc(row.external_group_id)}" title="Delete">${icons.trash('', 16)}</button>`;
        return `${edit}${del}`;
      }},
    ];

    this.#loadLookups();
  }

  async #loadLookups() {
    try {
      const depts = await window.fetchDepartmentList?.();
      this.#departments = Array.isArray(depts) ? depts : [];
    } catch { this.#departments = []; }
    try {
      const teams = await window.fetchTeamList?.();
      this.#teams = Array.isArray(teams) ? teams : [];
    } catch { this.#teams = []; }

    const deptSelect = this.querySelector('#mapping-department');
    for (const d of this.#departments) {
      const opt = document.createElement('option');
      opt.value = d.id; opt.textContent = d.name;
      deptSelect.appendChild(opt);
    }
    const teamSelect = this.querySelector('#mapping-team');
    for (const t of this.#teams) {
      const opt = document.createElement('option');
      opt.value = t.id; opt.textContent = t.name;
      teamSelect.appendChild(opt);
    }
    this.querySelector('#mappings-table').refresh();
  }

  #deptName(id) {
    if (!id) return '<span class="muted">--</span>';
    const d = this.#departments.find((x) => x.id === id);
    return this.#esc(d ? d.name : id);
  }

  #teamName(id) {
    if (!id) return '<span class="muted">--</span>';
    const t = this.#teams.find((x) => x.id === id);
    return this.#esc(t ? t.name : id);
  }

  #resetForm() {
    this.querySelector('#mapping-group-id').value = '';
    this.querySelector('#mapping-role').value = '';
    this.querySelector('#mapping-department').value = '';
    this.querySelector('#mapping-team').value = '';
    this.querySelector('#mapping-description').value = '';
  }

  #bind() {
    const table = this.querySelector('#mappings-table');
    const modal = this.querySelector('#mapping-modal');
    let editingId = null;

    this.querySelector('#btn-create').addEventListener('click', () => {
      editingId = null;
      modal.setAttribute('heading', 'New Group Mapping');
      this.#resetForm();
      modal.open();
    });
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());

    const syncBtn = this.querySelector('#btn-sync-azure');
    syncBtn.addEventListener('click', async () => {
      syncBtn.setAttribute('loading', '');
      try {
        const res = await window.syncAzureDirectory();
        if (!res.ok) throw new Error(await res.text());
        const summary = await res.json();
        const errorCount = (summary.errors || []).length;
        showToast(
          `Synced: ${summary.departments_created} department(s), ${summary.teams_created} team(s), ` +
          `${summary.users_created} new user(s) (${summary.users_skipped_existing} already synced)` +
          (errorCount ? ` — ${errorCount} error(s), see console` : '')
        );
        if (errorCount) console.warn('directory sync errors:', summary.errors);
      } catch (e) {
        showToast(`Directory sync failed: ${e.message}`);
      } finally {
        syncBtn.removeAttribute('loading');
      }
    });

    const saveBtn = this.querySelector('#btn-save');
    saveBtn.addEventListener('click', async () => {
      saveBtn.setAttribute('loading', '');
      try {
        const external_group_id = this.querySelector('#mapping-group-id').value.trim();
        const role = this.querySelector('#mapping-role').value || null;
        const department_id = this.querySelector('#mapping-department').value || null;
        const team_id = this.querySelector('#mapping-team').value || null;
        const description = this.querySelector('#mapping-description').value.trim() || null;

        if (!external_group_id) { showToast('Group Object ID is required'); return; }
        if (!role && !department_id && !team_id) { showToast('Set at least one of Role / Department / Team'); return; }

        let res;
        if (editingId) {
          // PUT distinguishes "omitted" (leave untouched) from "explicitly
          // cleared" via clear_role/clear_team/clear_department flags —
          // sending null for an emptied select would otherwise be a no-op.
          const body = { description };
          if (role) body.role = role; else body.clear_role = true;
          if (department_id) body.department_id = department_id; else body.clear_department = true;
          if (team_id) body.team_id = team_id; else body.clear_team = true;
          res = await window.updateGroupMapping(editingId, body);
        } else {
          res = await window.createGroupMapping({ external_group_id, role, department_id, team_id, description });
        }

        if (!res.ok) throw new Error(await res.text());
        modal.close();
        this.#resetForm();
        table.refresh();
        showToast(editingId ? 'Mapping updated' : 'Mapping created');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        saveBtn.removeAttribute('loading');
      }
    });

    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('[data-action]');
      if (!btn) return;
      const { action, id, group } = btn.dataset;

      if (action === 'edit') {
        editingId = id;
        modal.setAttribute('heading', 'Edit Group Mapping');
        this.querySelector('#mapping-group-id').value = btn.dataset.externalGroupId || '';
        this.querySelector('#mapping-role').value = btn.dataset.role || '';
        this.querySelector('#mapping-department').value = btn.dataset.departmentId || '';
        this.querySelector('#mapping-team').value = btn.dataset.teamId || '';
        this.querySelector('#mapping-description').value = btn.dataset.description || '';
        modal.open();
      } else if (action === 'delete') {
        if (!confirm(`Delete the mapping for group "${group}"?`)) return;
        try {
          const res = await window.deleteGroupMapping(id);
          if (!res.ok) throw new Error(res.statusText);
          table.refresh();
          showToast('Mapping deleted');
        } catch (err) { showToast(`Failed: ${err.message}`); }
      }
    });
  }

  #esc(value) {
    if (value == null) return '';
    return String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }
}

customElements.define('group-mappings-page', GroupMappingsPage);
