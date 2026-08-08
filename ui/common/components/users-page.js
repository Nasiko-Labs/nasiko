import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import '/common/components/app-empty-state.js';

import styles from './users-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

// Server user_role enum — values must match exactly or POST/PUT /users 500s.
const ROLE_LABELS = {
  admin: 'Admin',
  department_manager: 'Department Manager',
  team_lead: 'Team Lead',
  team_member: 'Team Member',
  member: 'Member',
};

function roleLabel(role) {
  if (!role) return 'Member';
  return ROLE_LABELS[role] || role;
}

function relativeTime(dateStr) {
  if (!dateStr) return 'Never';
  const d = new Date(dateStr);
  const now = new Date();
  const diffMs = now - d;
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));
  if (diffDays === 0) return 'Today';
  if (diffDays === 1) return 'Yesterday';
  if (diffDays < 7) return `${diffDays}d ago`;
  if (diffDays < 30) return `${Math.floor(diffDays / 7)}w ago`;
  return d.toLocaleDateString();
}

class UsersPage extends HTMLElement {
  #initialized = false;
  #departments = [];
  #teams = [];

  async connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    // EE: this page's APIs (fetchUsers/fetchUserStats) are superuser-only
    // and degrade to `{available: false}` (200, not 403) for an
    // authenticated-but-non-superuser caller (e.g. an org-hierarchy
    // `admin` who isn't a platform superuser) — see
    // ee/server/src/users.rs's `degradable_router()`. OSS's plain
    // `fetchUserStats` never returns this shape, so this check is a no-op
    // there. Checked via stats (cheap) rather than the full user list.
    if (typeof window.fetchUserStats === 'function') {
      try {
        const stats = await window.fetchUserStats();
        if (stats && stats.available === false) {
          this.innerHTML = `
            <div class="users-unavailable">
              <app-empty-state
                title="Not available for your role"
                description="Full user management requires platform superuser access. Ask a superuser to manage users, or use Team Access to manage roles within your own team/department.">
              </app-empty-state>
            </div>
          `;
          return;
        }
      } catch {
        // Stats endpoint unreachable — fall through to the normal page;
        // the table's own fetch will surface whatever the real error is.
      }
    }

    await this.#loadLookups();
    this.#render();
    this.#bind();
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
  }

  #placementLabel(row) {
    if (row.team_id) {
      const team = this.#teams.find((t) => t.id === row.team_id);
      return team ? this.#esc(team.name) : '<span class="muted">Unknown team</span>';
    }
    if (row.department_id) {
      const dept = this.#departments.find((d) => d.id === row.department_id);
      return dept ? this.#esc(dept.name) : '<span class="muted">Unknown department</span>';
    }
    return '<span class="muted">— Unassigned —</span>';
  }

  #render() {
    this.innerHTML = `
      <div class="page-head">
        <h1 class="title-page">Users</h1>
        <span class="count-chips" id="users-stats"></span>
        <div class="head-actions">
          <app-button variant="secondary" size="sm" id="btn-sync-azure">${icons.refresh('', 16)} Sync Azure AD Directory</app-button>
          <app-button variant="primary" size="sm" id="btn-add-user">${icons.plus('', 16)} Create user</app-button>
        </div>
      </div>

      <div class="filters-bar">
        <select class="filter-select" id="filter-role" aria-label="Filter by role">
          <option value="">All Roles</option>
          <option value="admin">Admin</option>
          <option value="department_manager">Department Manager</option>
          <option value="team_lead">Team Lead</option>
          <option value="team_member">Team Member</option>
          <option value="member">Member</option>
        </select>
        <select class="filter-select" id="filter-department" aria-label="Filter by department">
          <option value="">All Departments</option>
        </select>
        <select class="filter-select" id="filter-status" aria-label="Filter by status">
          <option value="">All Status</option>
          <option value="active">Active</option>
          <option value="disabled">Disabled</option>
        </select>
      </div>

      <smart-table id="users-table" data-fn="fetchUsers" search search-placeholder="Search users..." limit="20"></smart-table>

      <app-modal id="user-modal" heading="Add User">
        <div class="modal-form" id="create-form">
          <div class="row">
            <div class="field">
              <label for="user-username">Username</label>
              <input type="text" id="user-username" placeholder="johndoe" required />
            </div>
            <div class="field">
              <label for="user-email">Email</label>
              <input type="email" id="user-email" placeholder="john@example.com" required />
            </div>
          </div>
          <div class="row">
            <div class="field">
              <label for="user-display-name">Display Name</label>
              <input type="text" id="user-display-name" placeholder="John Doe" />
            </div>
            <div class="field">
              <label for="user-role">Role</label>
              <select id="user-role">
                <option value="member" selected>Member</option>
                <option value="team_member">Team Member</option>
                <option value="team_lead">Team Lead</option>
                <option value="department_manager">Department Manager</option>
                <option value="admin">Admin</option>
              </select>
            </div>
          </div>
          <p class="hint">A one-time access secret is generated on create — no password needed.</p>
          <div class="form-actions" data-slot="footer">
            <app-button variant="secondary" id="btn-cancel">Cancel</app-button>
            <app-button variant="primary" id="btn-save">Create</app-button>
          </div>
        </div>
        <div class="modal-form credentials-view" id="create-credentials" hidden>
          <p class="hint cred-warning" id="cred-message"></p>
          <div class="field">
            <label>Username</label>
            <div class="cred-value">
              <code id="cred-username"></code>
              <button type="button" class="action-btn" data-copy="cred-username" title="Copy">${icons.copy('', 16)}</button>
            </div>
          </div>
          <div class="field">
            <label>Access Key</label>
            <div class="cred-value">
              <code id="cred-access-key"></code>
              <button type="button" class="action-btn" data-copy="cred-access-key" title="Copy">${icons.copy('', 16)}</button>
            </div>
          </div>
          <div class="field">
            <label>Access Secret</label>
            <div class="cred-value">
              <code id="cred-access-secret"></code>
              <button type="button" class="action-btn" data-copy="cred-access-secret" title="Copy">${icons.copy('', 16)}</button>
            </div>
          </div>
          <div class="form-actions">
            <app-button variant="primary" id="btn-done">Done</app-button>
          </div>
        </div>
      </app-modal>

      <app-modal id="edit-modal" heading="Edit User">
        <div class="modal-form">
          <div class="row">
            <div class="field">
              <label for="edit-username">Username</label>
              <input type="text" id="edit-username" />
            </div>
            <div class="field">
              <label for="edit-email">Email</label>
              <input type="email" id="edit-email" />
            </div>
          </div>
          <div class="row">
            <div class="field">
              <label for="edit-display-name">Display Name</label>
              <input type="text" id="edit-display-name" />
            </div>
            <div class="field">
              <label for="edit-password">New Password</label>
              <input type="password" id="edit-password" placeholder="leave blank to keep" />
            </div>
          </div>
          <p class="hint">Leave password blank to keep the current one.</p>
          <div class="form-actions" data-slot="footer">
            <app-button variant="secondary" id="edit-cancel">Cancel</app-button>
            <app-button variant="primary" id="edit-save">Save</app-button>
          </div>
        </div>
      </app-modal>

      <app-modal id="reassign-modal" heading="Assign department / team">
        <div class="modal-form">
          <p class="hint" id="reassign-target"></p>
          <div class="field">
            <label for="reassign-department">Department</label>
            <select id="reassign-department"><option value="">— Unassigned —</option></select>
          </div>
          <div class="field">
            <label for="reassign-team">Team <span class="hint">(optional — leave unset for a department-only placement)</span></label>
            <select id="reassign-team"><option value="">— No specific team —</option></select>
          </div>
          <div class="form-actions" data-slot="footer">
            <app-button variant="secondary" id="reassign-cancel">Cancel</app-button>
            <app-button variant="primary" id="reassign-save">Save</app-button>
          </div>
        </div>
      </app-modal>
    `;

    const table = this.querySelector('#users-table');
    table.columns = [
      { key: 'display_name', label: 'User', width: '24%', render: (v, row) => {
        const name = v || row.username;
        const initials = name.split(/\s+/).map((p) => p[0]).slice(0, 2).join('');
        return `<div class="user-cell">
          <span class="user-avatar">${this.#esc(initials)}</span>
          <span>
            <span class="user-name">${this.#esc(name)}</span><br>
            <span class="user-email">${this.#esc(row.email)}</span>
          </span>
        </div>`;
      }},
      { key: 'role', label: 'Role', width: '10%', render: (v, row) => {
        if (row.is_superuser) return '<span class="badge badge--warning">superuser</span>';
        const variant = v === 'admin' ? 'info'
          : (v === 'department_manager' || v === 'team_lead') ? 'success'
          : 'neutral';
        return `<span class="badge badge--${variant}">${this.#esc(roleLabel(v))}</span>`;
      }},
      { key: 'department_team', label: 'Department / Team', width: '14%', render: (_, row) => this.#placementLabel(row) },
      { key: 'is_active', label: 'Status', width: '9%', render: (v) =>
        v ? '<span class="badge badge--success">Active</span>'
          : '<span class="badge badge--muted">Disabled</span>'
      },
      { key: 'last_login', label: 'Last Active', width: '12%', render: (v) =>
        `<span title="${v || 'Never'}">${relativeTime(v)}</span>`
      },
      { key: 'created_at', label: 'Created', width: '9%', render: (v) => v ? new Date(v).toLocaleDateString() : '--' },
      { key: 'actions', label: '', width: '14%', render: (_, row) => {
        if (row.is_superuser) return '';
        const esc = (s) => (s || '').replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        const edit = `<button class="action-btn" data-action="edit" data-id="${esc(row.id)}" data-username="${esc(row.username)}" data-email="${esc(row.email)}" data-display-name="${esc(row.display_name || '')}" title="Edit">${icons.edit('', 16)}</button>`;
        const reassign = `<button class="action-btn" data-action="reassign" data-id="${esc(row.id)}" data-name="${esc(row.display_name || row.username)}" data-department-id="${esc(row.department_id || '')}" data-team-id="${esc(row.team_id || '')}" title="Assign department/team">${icons.folder('', 16)}</button>`;
        const deactivate = row.is_active
          ? `<button class="action-btn action-btn--warn" data-action="deactivate" data-id="${esc(row.id)}" title="Deactivate">${icons.xCircle('', 16)}</button>`
          : `<button class="action-btn" data-action="activate" data-id="${esc(row.id)}" title="Activate">${icons.checkCircle('', 16)}</button>`;
        return `${edit}${reassign}${deactivate}<button class="action-btn action-btn--danger" data-action="delete" data-id="${esc(row.id)}" data-name="${esc(row.username)}" title="Delete">${icons.trash('', 16)}</button>`;
      }},
    ];

    const deptSelect = this.querySelector('#reassign-department');
    for (const d of this.#departments) {
      const opt = document.createElement('option');
      opt.value = d.id; opt.textContent = d.name;
      deptSelect.appendChild(opt);
    }
    const teamSelect = this.querySelector('#reassign-team');
    for (const t of this.#teams) {
      const opt = document.createElement('option');
      opt.value = t.id; opt.textContent = t.name;
      opt.dataset.departmentId = t.department_id || '';
      teamSelect.appendChild(opt);
    }
  }

  #bind() {
    const table = this.querySelector('#users-table');
    const modal = this.querySelector('#user-modal');
    const editModal = this.querySelector('#edit-modal');
    const reassignModal = this.querySelector('#reassign-modal');
    let editingId = null;
    let reassigningId = null;

    const reassignDept = this.querySelector('#reassign-department');
    const reassignTeam = this.querySelector('#reassign-team');

    const filterTeamsByDepartment = () => {
      const deptId = reassignDept.value;
      for (const opt of reassignTeam.options) {
        if (!opt.value) continue;
        opt.hidden = Boolean(deptId) && opt.dataset.departmentId !== deptId;
      }
      // Clear a team selection that no longer belongs to the picked department.
      if (reassignTeam.selectedOptions[0]?.hidden) reassignTeam.value = '';
    };
    reassignDept.addEventListener('change', filterTeamsByDepartment);

    this.querySelector('#reassign-cancel').addEventListener('click', () => reassignModal.close());

    const reassignSaveBtn = this.querySelector('#reassign-save');
    reassignSaveBtn.addEventListener('click', async () => {
      if (!reassigningId) return;
      reassignSaveBtn.setAttribute('loading', '');
      try {
        const teamId = reassignTeam.value || null;
        const departmentId = reassignDept.value || null;
        const res = await window.updateUserPlacement(reassigningId, {
          teamId,
          departmentId,
          clear: !teamId && !departmentId,
        });
        if (!res.ok) throw new Error(await res.text());
        reassignModal.close();
        table.refresh();
        showToast('Assignment updated');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        reassignSaveBtn.removeAttribute('loading');
      }
    });

    // Stats update after table loads
    table.addEventListener('loading-end', () => {
      this.#updateStats();
    });

    // Filters
    const filterRole = this.querySelector('#filter-role');
    const filterDept = this.querySelector('#filter-department');
    const filterStatus = this.querySelector('#filter-status');

    const applyFilters = () => {
      const role = filterRole.value;
      const dept = filterDept.value;
      const status = filterStatus.value;

      // Wrap the original fetchUsers with filters
      const baseFn = window._fetchUsersBase || window.fetchUsers;
      if (!window._fetchUsersBase) window._fetchUsersBase = window.fetchUsers;

      window.fetchUsers = async (query, page, limit) => {
        const params = new URLSearchParams();
        if (query) params.set('q', query);
        if (role) params.set('role', role);
        if (dept) params.set('department', dept);
        if (status) params.set('status', status);
        params.set('limit', limit);
        params.set('offset', (page - 1) * limit);
        const res = await apiFetch(`/users?${params}`);
        if (!res.ok) throw new Error(res.statusText);
        return res.json();
      };
      table.refresh();
    };

    filterRole.addEventListener('change', applyFilters);
    filterDept.addEventListener('change', applyFilters);
    filterStatus.addEventListener('change', applyFilters);

    // Load departments for filter
    if (window.fetchDepartmentList) {
      window.fetchDepartmentList().then(depts => {
        for (const d of depts) {
          const opt = document.createElement('option');
          opt.value = d.id || d.name;
          opt.textContent = d.name;
          filterDept.appendChild(opt);
        }
      }).catch(() => {});
    }

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
          `${summary.users_created} new user(s) (${summary.users_skipped_existing} already in the system)` +
          (errorCount ? ` — ${errorCount} error(s), see console` : '')
        );
        if (errorCount) console.warn('directory sync errors:', summary.errors);
        table.refresh();
      } catch (e) {
        showToast(`Directory sync failed: ${e.message}`);
      } finally {
        syncBtn.removeAttribute('loading');
      }
    });

    // Create modal — POST /users takes no password; the server returns
    // one-time credentials (access_key/access_secret) in the 201 body,
    // shown once in the credentials view before the modal is dismissed.
    const resetCreateModal = () => {
      this.querySelector('#user-username').value = '';
      this.querySelector('#user-email').value = '';
      this.querySelector('#user-display-name').value = '';
      this.querySelector('#user-role').value = 'member';
      this.querySelector('#create-form').hidden = false;
      this.querySelector('#create-credentials').hidden = true;
      modal.setAttribute('heading', 'Add User');
    };

    this.querySelector('#btn-add-user').addEventListener('click', () => modal.open());
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());
    this.querySelector('#btn-done').addEventListener('click', () => modal.close());
    // Reset on every close (X, backdrop, Cancel, Done) so a reopened modal
    // never shows stale credentials. `close` doesn't bubble past <dialog>.
    modal.querySelector('dialog').addEventListener('close', resetCreateModal);

    const createBtn = this.querySelector('#btn-save');
    createBtn.addEventListener('click', async () => {
      createBtn.setAttribute('loading', '');
      try {
        const username = this.querySelector('#user-username').value.trim();
        const email = this.querySelector('#user-email').value.trim();
        const display_name = this.querySelector('#user-display-name').value.trim();
        const role = this.querySelector('#user-role').value;
        if (!username || !email) { showToast('Username and email are required'); return; }

        const res = await apiFetch('/users', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username, email, display_name: display_name || null, role }),
        });
        if (!res.ok) throw new Error(await res.text());
        const created = await res.json();

        this.querySelector('#cred-username').textContent = created.username || username;
        this.querySelector('#cred-access-key').textContent = created.access_key || '';
        this.querySelector('#cred-access-secret').textContent = created.access_secret || '';
        this.querySelector('#cred-message').textContent =
          created.message || "Store access_secret securely — it won't be shown again.";
        this.querySelector('#create-form').hidden = true;
        this.querySelector('#create-credentials').hidden = false;
        modal.setAttribute('heading', 'User Created');
        table.refresh();
        showToast('User created');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        createBtn.removeAttribute('loading');
      }
    });

    // Copy-to-clipboard for the one-time credentials view
    this.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-copy]');
      if (!btn) return;
      const target = this.querySelector(`#${btn.dataset.copy}`);
      if (!target) return;
      navigator.clipboard.writeText(target.textContent).catch(() => {});
      btn.innerHTML = icons.check('', 16);
      setTimeout(() => { btn.innerHTML = icons.copy('', 16); }, 1500);
    });

    // Edit modal
    this.querySelector('#edit-cancel').addEventListener('click', () => editModal.close());

    const editSaveBtn = this.querySelector('#edit-save');
    editSaveBtn.addEventListener('click', async () => {
      if (!editingId) return;
      editSaveBtn.setAttribute('loading', '');
      try {
        const username = this.querySelector('#edit-username').value.trim();
        const email = this.querySelector('#edit-email').value.trim();
        const display_name = this.querySelector('#edit-display-name').value.trim();
        const password = this.querySelector('#edit-password').value;
        if (!username || !email) { showToast('Username and email are required'); return; }
        if (password && password.length < 8) { showToast('Password must be at least 8 characters'); return; }

        const body = { username, email, display_name: display_name || null };
        if (password) body.password = password;

        const res = await apiFetch(`/users/${editingId}`, {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });
        if (!res.ok) throw new Error(await res.text());
        editModal.close();
        table.refresh();
        showToast('User updated');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        editSaveBtn.removeAttribute('loading');
      }
    });

    // Action clicks
    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('[data-action]');
      if (!btn) return;
      const { action, id, name } = btn.dataset;

      if (action === 'edit') {
        editingId = id;
        this.querySelector('#edit-username').value = btn.dataset.username || '';
        this.querySelector('#edit-email').value = btn.dataset.email || '';
        this.querySelector('#edit-display-name').value = btn.dataset.displayName || '';
        this.querySelector('#edit-password').value = '';
        editModal.open();
      } else if (action === 'reassign') {
        reassigningId = id;
        this.querySelector('#reassign-target').textContent = `Assigning: ${btn.dataset.name || 'this user'}`;
        reassignDept.value = btn.dataset.departmentId || '';
        reassignTeam.value = btn.dataset.teamId || '';
        filterTeamsByDepartment();
        reassignModal.open();
      } else if (action === 'delete') {
        if (!confirm(`Delete user "${name}"? This cannot be undone.`)) return;
        try {
          const res = await apiFetch(`/users/${id}`, { method: 'DELETE' });
          if (!res.ok) throw new Error(res.statusText);
          table.refresh();
          showToast('User deleted');
        } catch (err) { showToast(`Failed: ${err.message}`); }
      } else if (action === 'deactivate' || action === 'activate') {
        const is_active = action === 'activate';
        try {
          const res = await apiFetch(`/users/${id}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ is_active }),
          });
          if (!res.ok) throw new Error(res.statusText);
          table.refresh();
          showToast(`User ${is_active ? 'activated' : 'deactivated'}`);
        } catch (err) { showToast(`Failed: ${err.message}`); }
      }
    });
  }

  #updateStats() {
    const el = this.querySelector('#users-stats');
    if (!el) return;
    if (window.fetchUserStats) {
      window.fetchUserStats().then(stats => {
        el.innerHTML = `<span class="count-chip"><b>${stats.admins}</b> admins</span>`
          + `<span class="count-chip"><b>${stats.total}</b> users</span>`;
      }).catch(() => {});
    }
  }

  #esc(value) {
    if (value == null) return '';
    return String(value)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }
}

customElements.define('users-page', UsersPage);
