import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-button.js';
import '/common/components/app-skeleton.js';
import '/common/components/app-modal.js';
import '/common/components/smart-table.js';

import styles from './access-control-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class AccessControlPage extends HTMLElement {
  #initialized = false;
  #data = null;
  #pickerDepartments = [];
  #pickerTeams = [];
  #assigningId = null;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    // Delegated + attached once (not per-#render(), since #render() is
    // re-invoked after a successful assign to refresh stats/warnings) —
    // looks up the modal/selects fresh via this.querySelector at click
    // time, so it stays correct across re-renders instead of accumulating
    // one listener per render.
    this.addEventListener('click', (e) => this.#onAssignClick(e));
    this.#setState('loading');
    this.#load();
  }

  #onAssignClick(e) {
    const btn = e.target.closest('[data-action="assign"]');
    if (!btn) return;
    this.#assigningId = btn.dataset.id;
    const deptSelect = this.querySelector('#assign-department');
    const teamSelect = this.querySelector('#assign-team');
    this.querySelector('#assign-target').textContent = `Assigning: ${btn.dataset.name || 'this user'}`;
    deptSelect.value = '';
    teamSelect.value = '';
    this.#filterTeamsByDepartment();
    this.querySelector('#assign-modal').open();
  }

  #filterTeamsByDepartment() {
    const deptSelect = this.querySelector('#assign-department');
    const teamSelect = this.querySelector('#assign-team');
    const deptId = deptSelect.value;
    for (const opt of teamSelect.options) {
      if (!opt.value) continue;
      opt.hidden = Boolean(deptId) && opt.dataset.departmentId !== deptId;
    }
    if (teamSelect.selectedOptions[0]?.hidden) teamSelect.value = '';
  }

  #setState(state, error) {
    if (state === 'loading') {
      this.innerHTML = `
        <div class="page-head" style="min-height:var(--control-h-lg)">
          <app-skeleton lines="1" height="1.75rem" style="flex:1"></app-skeleton>
        </div>
        <div class="attn-band" style="min-height:88px">
          <div class="attn-item"><app-skeleton lines="2" height="1rem"></app-skeleton></div>
          <div class="attn-item"><app-skeleton lines="2" height="1rem"></app-skeleton></div>
          <div class="attn-item"><app-skeleton lines="2" height="1rem"></app-skeleton></div>
        </div>
        <div class="section">
          <app-skeleton lines="1" height="1.5rem"></app-skeleton>
          <app-skeleton lines="5" height="1rem"></app-skeleton>
        </div>
      `;
    } else if (state === 'error') {
      this.innerHTML = `
        <div class="error-message">${this.#esc(error || 'Failed to load data.')}</div>
      `;
    } else if (state === 'success') {
      this.#render();
    }
  }

  async #load() {
    try {
      if (typeof window.fetchAccessControlOverview !== 'function') {
        throw new Error('fetchAccessControlOverview is not defined');
      }
      this.#data = await window.fetchAccessControlOverview();
      try {
        const depts = await window.fetchDepartmentList?.();
        this.#pickerDepartments = Array.isArray(depts) ? depts : [];
      } catch { this.#pickerDepartments = []; }
      try {
        const teams = await window.fetchTeamList?.();
        this.#pickerTeams = Array.isArray(teams) ? teams : [];
      } catch { this.#pickerTeams = []; }
      this.#setState('success');
    } catch (e) {
      this.#setState('error', e.message);
    }
  }

  #render() {
    const { stats, warnings, departments, unassigned_users } = this.#data;

    // Register data functions for smart-table
    window._acpFetchDepartments = async () => ({
      data: departments || [],
      total: (departments || []).length,
    });

    window._acpFetchUnassigned = async () => ({
      data: unassigned_users || [],
      total: (unassigned_users || []).length,
    });

    const countChip = (value, label) =>
      `<span class="count-chip"><b>${this.#fmtNum(value)}</b> ${label}</span>`;

    this.innerHTML = `
      <div class="page-head">
        <h1 class="title-page">Access control</h1>
        <div class="count-chips">
          ${countChip(stats.admins, 'admins')}
          ${countChip(stats.departments, 'departments')}
          ${countChip(stats.teams, 'teams')}
          ${countChip(stats.users, 'users')}
          ${countChip(stats.agents, 'agents')}
        </div>
        <div class="head-actions">
          <app-button variant="primary" size="sm" id="btn-create-user">
            ${icons.plus('', 14)} Create user
          </app-button>
          <app-button variant="secondary" size="sm" id="btn-create-team">
            ${icons.plus('', 14)} Create team
          </app-button>
        </div>
      </div>

      ${(warnings || []).length > 0 ? `
      <div class="attn-band">
        ${warnings.map(w => `
          <div class="attn-item${w.count > 0 && w.type !== 'info' ? ' is-alert' : ''}">
            <span class="attn-label">${this.#esc(w.label)}</span>
            <span class="attn-value">${this.#fmtNum(w.count)}</span>
          </div>
        `).join('')}
      </div>` : ''}

      <div class="section">
        <div class="section-header">
          <h2>Departments</h2>
        </div>
        <smart-table id="departments-table" data-fn="_acpFetchDepartments" limit="10"></smart-table>
      </div>

      <div class="section">
        <div class="section-header">
          <h2>Unassigned members</h2>
        </div>
        <smart-table id="unassigned-table" data-fn="_acpFetchUnassigned" limit="10"></smart-table>
      </div>

      <app-modal id="assign-modal" heading="Assign department / team">
        <div class="modal-form">
          <p class="hint" id="assign-target"></p>
          <div class="field">
            <label for="assign-department">Department</label>
            <select id="assign-department"><option value="">— Unassigned —</option></select>
          </div>
          <div class="field">
            <label for="assign-team">Team <span class="hint">(optional — leave unset for a department-only placement)</span></label>
            <select id="assign-team"><option value="">— No specific team —</option></select>
          </div>
          <div class="form-actions" data-slot="footer">
            <app-button variant="secondary" id="assign-cancel">Cancel</app-button>
            <app-button variant="primary" id="assign-save">Save</app-button>
          </div>
        </div>
      </app-modal>
    `;

    // Configure departments table columns
    const warnCell = (label) =>
      `<span class="cell-warn">${icons.info('', 14)} ${label}</span>`;
    const deptStatus = (row) => {
      const teams = row.teams_count ?? 0;
      if (!teams) return '<span class="badge badge--warning">Needs setup</span>';
      if (!row.manager) return '<span class="badge badge--error">Needs attention</span>';
      return '<span class="badge badge--success">Fully configured</span>';
    };
    const deptTable = this.querySelector('#departments-table');
    deptTable.columns = [
      { key: 'name', label: 'Name', width: '22%', render: (v) => `<strong>${this.#esc(v)}</strong>` },
      { key: 'manager', label: 'Manager', width: '20%', render: (v) =>
        v ? this.#esc(v) : warnCell('No manager assigned') },
      { key: 'teams_count', label: 'Teams', width: '14%', render: (v) =>
        (v ?? 0) > 0
          ? `<span class="tag-chip">${this.#fmtNum(v)} team${v === 1 ? '' : 's'}</span>`
          : warnCell('No teams') },
      { key: 'members_count', label: 'Members', width: '13%', render: (v) => String(v ?? 0) },
      { key: 'agents_count', label: 'Agents', width: '13%', render: (v) => String(v ?? 0) },
      { key: 'status', label: 'Status', width: '18%', render: (_, row) => deptStatus(row) },
    ];

    // Configure unassigned users table columns
    const unassignedTable = this.querySelector('#unassigned-table');
    unassignedTable.columns = [
      { key: 'display_name', label: 'User', width: '26%', render: (v, row) => {
        const name = v || row.username || '—';
        const initials = name.split(/\s+/).map((p) => p[0]).slice(0, 2).join('');
        return `<div class="user-cell">
          <span class="user-avatar">${this.#esc(initials)}</span>
          <span>
            <span class="user-name">${this.#esc(name)}</span><br>
            <span class="user-email">${this.#esc(row.email || '')}</span>
          </span>
        </div>`;
      }},
      { key: 'role', label: 'Role', width: '16%', render: (v) => {
        const variant = v === 'admin' ? 'info' : v === 'deployer' ? 'success' : 'neutral';
        return `<span class="badge badge--${variant}">${this.#esc(v || 'member')}</span>`;
      }},
      { key: 'created_at', label: 'Created', width: '16%', render: (v) =>
        v ? new Date(v).toLocaleDateString() : '—'
      },
      { key: 'actions', label: '', width: '12%', render: (_, row) => {
        const esc = (s) => (s == null ? '' : String(s)).replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        return `<button class="row-action" data-action="assign" data-id="${esc(row.id)}" data-name="${esc(row.display_name || row.username)}" title="Assign department/team">Assign</button>`;
      }},
    ];

    // Populate the assign-modal pickers
    const deptSelect = this.querySelector('#assign-department');
    for (const d of this.#pickerDepartments) {
      const opt = document.createElement('option');
      opt.value = d.id; opt.textContent = d.name;
      deptSelect.appendChild(opt);
    }
    const teamSelect = this.querySelector('#assign-team');
    for (const t of this.#pickerTeams) {
      const opt = document.createElement('option');
      opt.value = t.id; opt.textContent = t.name;
      opt.dataset.departmentId = t.department_id || '';
      teamSelect.appendChild(opt);
    }
    deptSelect.addEventListener('change', () => this.#filterTeamsByDepartment());

    const assignModal = this.querySelector('#assign-modal');
    this.querySelector('#assign-cancel').addEventListener('click', () => assignModal.close());

    const assignSaveBtn = this.querySelector('#assign-save');
    assignSaveBtn.addEventListener('click', async () => {
      if (!this.#assigningId) return;
      assignSaveBtn.setAttribute('loading', '');
      try {
        const teamId = teamSelect.value || null;
        const departmentId = deptSelect.value || null;
        const res = await window.updateUserPlacement(this.#assigningId, {
          teamId,
          departmentId,
          clear: !teamId && !departmentId,
        });
        if (!res.ok) throw new Error(await res.text());
        assignModal.close();
        // Re-fetch the overview so the "Unassigned Members" table (and the
        // warning counts above it) reflect the new placement immediately.
        this.#data = await window.fetchAccessControlOverview();
        this.#render();
        showToast('Assignment updated');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        assignSaveBtn.removeAttribute('loading');
      }
    });

    // Button events
    this.querySelector('#btn-create-user')?.addEventListener('click', () => {
      window.location.href = '/users.html';
    });

    this.querySelector('#btn-create-team')?.addEventListener('click', () => {
      showToast('Team creation coming soon');
    });
  }

  #fmtNum(n) {
    return n != null ? Number(n).toLocaleString() : '0';
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('access-control-page', AccessControlPage);
