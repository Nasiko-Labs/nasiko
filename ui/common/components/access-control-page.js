import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-button.js';
import '/common/components/app-badge.js';
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
        <div class="stats-bar" style="min-height:72px">
          <app-skeleton lines="1" height="2rem"></app-skeleton>
        </div>
        <div class="warnings-grid">
          <app-skeleton lines="2" height="1rem"></app-skeleton>
          <app-skeleton lines="2" height="1rem"></app-skeleton>
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

    const activeWarnings = (warnings || []).filter(w => w.count > 0);

    // Register data functions for smart-table
    window._acpFetchDepartments = async () => ({
      data: departments || [],
      total: (departments || []).length,
    });

    window._acpFetchUnassigned = async () => ({
      data: unassigned_users || [],
      total: (unassigned_users || []).length,
    });

    this.innerHTML = `
      <div class="stats-bar">
        <div class="stat-item">
          <span class="stat-value">${this.#fmtNum(stats.admins)}</span>
          <span class="stat-label">Admins</span>
        </div>
        <div class="stat-item">
          <span class="stat-value">${this.#fmtNum(stats.departments)}</span>
          <span class="stat-label">Departments</span>
        </div>
        <div class="stat-item">
          <span class="stat-value">${this.#fmtNum(stats.teams)}</span>
          <span class="stat-label">Teams</span>
        </div>
        <div class="stat-item">
          <span class="stat-value">${this.#fmtNum(stats.users)}</span>
          <span class="stat-label">Users</span>
        </div>
        <div class="stat-item">
          <span class="stat-value">${this.#fmtNum(stats.agents)}</span>
          <span class="stat-label">Agents</span>
        </div>
      </div>

      ${activeWarnings.length > 0 ? `
      <div class="warnings-grid">
        ${activeWarnings.map(w => `
          <div class="warning-card is-${this.#escAttr(w.type)}">
            <span class="warning-count">${this.#fmtNum(w.count)}</span>
            <span class="warning-label">${this.#esc(w.label)}</span>
          </div>
        `).join('')}
      </div>` : ''}

      <div class="section">
        <div class="section-header">
          <h2>Departments</h2>
          <app-button variant="secondary" size="sm" id="btn-create-team">
            ${icons.plus('', 14)} Create Team
          </app-button>
        </div>
        <smart-table id="departments-table" data-fn="_acpFetchDepartments" limit="10"></smart-table>
      </div>

      <div class="section">
        <div class="section-header">
          <h2>Unassigned Members</h2>
          <app-button variant="secondary" size="sm" id="btn-create-user">
            ${icons.plus('', 14)} Create User
          </app-button>
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
    const deptTable = this.querySelector('#departments-table');
    deptTable.columns = [
      { key: 'name', label: 'Name', width: '25%' },
      { key: 'manager', label: 'Manager', width: '20%', render: (v) => v || '—' },
      { key: 'teams_count', label: 'Teams', width: '15%', render: (v) => String(v ?? 0) },
      { key: 'members_count', label: 'Members', width: '20%', render: (v) => String(v ?? 0) },
      { key: 'agents_count', label: 'Agents', width: '20%', render: (v) => String(v ?? 0) },
    ];

    // Configure unassigned users table columns
    const unassignedTable = this.querySelector('#unassigned-table');
    unassignedTable.columns = [
      { key: 'username', label: 'Username', width: '18%' },
      { key: 'display_name', label: 'Name', width: '18%', render: (v) => v || '—' },
      { key: 'email', label: 'Email', width: '24%', render: (v) => v || '—' },
      { key: 'role', label: 'Role', width: '14%', render: (v) => {
        const variant = v === 'admin' ? 'info' : v === 'deployer' ? 'success' : 'neutral';
        return `<app-badge variant="${variant}">${v || 'member'}</app-badge>`;
      }},
      { key: 'created_at', label: 'Created', width: '10%', render: (v) =>
        v ? new Date(v).toLocaleDateString() : '—'
      },
      { key: 'actions', label: '', width: '4%', render: (_, row) => {
        const esc = (s) => (s == null ? '' : String(s)).replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        return `<button class="action-btn" data-action="assign" data-id="${esc(row.id)}" data-name="${esc(row.display_name || row.username)}" title="Assign department/team">${icons.folder('', 16)}</button>`;
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

  #escAttr(s) {
    return (s || '').replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }
}

customElements.define('access-control-page', AccessControlPage);
