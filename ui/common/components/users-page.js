import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';
import '/common/components/app-modal.js';
import '/common/components/app-badge.js';
import '/common/components/app-button.js';
import '/common/components/app-empty-state.js';

import styles from './users-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const AVATAR_COLORS = [
  '#3b6fd4', '#059669', '#d97706', '#7c3aed',
  '#dc2626', '#0891b2', '#4f46e5', '#be185d',
];

function avatarColor(name) {
  let hash = 0;
  const str = name || '';
  for (let i = 0; i < str.length; i++) {
    hash = str.charCodeAt(i) + ((hash << 5) - hash);
  }
  return AVATAR_COLORS[Math.abs(hash) % AVATAR_COLORS.length];
}

function avatarLetter(name) {
  if (!name) return '?';
  return name.charAt(0).toUpperCase();
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

    this.#render();
    this.#bind();
  }

  #render() {
    this.innerHTML = `
      <div class="users-header">
        <span class="users-header-stats" id="users-stats"></span>
        <app-button variant="primary" size="sm" id="btn-add-user">${icons.plus('', 16)} Add User</app-button>
      </div>

      <div class="filters-bar">
        <select class="filter-select" id="filter-role" aria-label="Filter by role">
          <option value="">All Roles</option>
          <option value="admin">Admin</option>
          <option value="deployer">Deployer</option>
          <option value="viewer">Viewer</option>
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
        <div class="modal-form">
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
              <label for="user-password">Password</label>
              <input type="password" id="user-password" placeholder="min 8 characters" required />
            </div>
            <div class="field">
              <label for="user-display-name">Display Name</label>
              <input type="text" id="user-display-name" placeholder="John Doe" />
            </div>
          </div>
          <div class="field">
            <label for="user-role">Role</label>
            <select id="user-role">
              <option value="viewer">Viewer</option>
              <option value="deployer">Deployer</option>
              <option value="admin">Admin</option>
            </select>
          </div>
          <div class="form-actions" data-slot="footer">
            <app-button variant="secondary" id="btn-cancel">Cancel</app-button>
            <app-button variant="primary" id="btn-save">Create</app-button>
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
    `;

    const table = this.querySelector('#users-table');
    table.columns = [
      { key: 'display_name', label: 'User', width: '24%', render: (v, row) => {
        const name = v || row.username;
        const color = avatarColor(name);
        const letter = avatarLetter(name);
        return `<div class="user-cell">
          <span class="user-avatar" style="background:${color}">${letter}</span>
          <span>
            <span class="user-name">${this.#esc(name)}</span><br>
            <span class="user-email">${this.#esc(row.email)}</span>
          </span>
        </div>`;
      }},
      { key: 'role', label: 'Role', width: '12%', render: (v, row) => {
        if (row.is_superuser) return '<app-badge variant="warning">superuser</app-badge>';
        const variant = v === 'admin' ? 'info' : v === 'deployer' ? 'success' : 'neutral';
        return `<app-badge variant="${variant}">${v || 'viewer'}</app-badge>`;
      }},
      { key: 'is_active', label: 'Status', width: '10%', render: (v) => {
        const cls = v ? 'is-active' : 'is-disabled';
        const label = v ? 'Active' : 'Disabled';
        return `<span class="status-cell"><span class="status-dot ${cls}"></span>${label}</span>`;
      }},
      { key: 'last_login', label: 'Last Active', width: '12%', render: (v) =>
        `<span title="${v || 'Never'}">${relativeTime(v)}</span>`
      },
      { key: 'created_at', label: 'Created', width: '10%', render: (v) => v ? new Date(v).toLocaleDateString() : '--' },
      { key: 'actions', label: '', width: '12%', render: (_, row) => {
        if (row.is_superuser) return '';
        const esc = (s) => (s || '').replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        const edit = `<button class="action-btn" data-action="edit" data-id="${esc(row.id)}" data-username="${esc(row.username)}" data-email="${esc(row.email)}" data-display-name="${esc(row.display_name || '')}" title="Edit">${icons.edit('', 16)}</button>`;
        const deactivate = row.is_active
          ? `<button class="action-btn action-btn--warn" data-action="deactivate" data-id="${esc(row.id)}" title="Deactivate">${icons.xCircle('', 16)}</button>`
          : `<button class="action-btn" data-action="activate" data-id="${esc(row.id)}" title="Activate">${icons.checkCircle('', 16)}</button>`;
        return `${edit}${deactivate}<button class="action-btn action-btn--danger" data-action="delete" data-id="${esc(row.id)}" data-name="${esc(row.username)}" title="Delete">${icons.trash('', 16)}</button>`;
      }},
    ];
  }

  #bind() {
    const table = this.querySelector('#users-table');
    const modal = this.querySelector('#user-modal');
    const editModal = this.querySelector('#edit-modal');
    let editingId = null;

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

    // Create modal
    this.querySelector('#btn-add-user').addEventListener('click', () => modal.open());
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());

    const createBtn = this.querySelector('#btn-save');
    createBtn.addEventListener('click', async () => {
      createBtn.setAttribute('loading', '');
      try {
        const username = this.querySelector('#user-username').value.trim();
        const email = this.querySelector('#user-email').value.trim();
        const password = this.querySelector('#user-password').value;
        const display_name = this.querySelector('#user-display-name').value.trim();
        const role = this.querySelector('#user-role').value;
        if (!username || !email) { showToast('Username and email are required'); return; }
        if (password.length < 8) { showToast('Password must be at least 8 characters'); return; }

        const res = await apiFetch('/users', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username, email, password, display_name: display_name || null, role }),
        });
        if (!res.ok) throw new Error(await res.text());
        modal.close();
        this.querySelector('#user-username').value = '';
        this.querySelector('#user-email').value = '';
        this.querySelector('#user-password').value = '';
        this.querySelector('#user-display-name').value = '';
        this.querySelector('#user-role').value = 'viewer';
        table.refresh();
        showToast('User created');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        createBtn.removeAttribute('loading');
      }
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
        el.innerHTML = `<span class="count">${stats.admins}</span> admins, <span class="count">${stats.total}</span> users`;
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
