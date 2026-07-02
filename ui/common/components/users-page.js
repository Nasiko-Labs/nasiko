import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';
import '/common/components/app-modal.js';
import '/common/components/app-badge.js';

import styles from './users-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class UsersPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <div class="actions-bar">
        <button class="add-btn" id="btn-add-user">${icons.plus('', 16)} Add User</button>
      </div>
      <smart-table id="users-table" data-fn="fetchUsers" search search-placeholder="Search users..." limit="20"></smart-table>

      <app-modal id="user-modal" heading="Add User">
        <div class="modal-form">
          <div class="row">
            <div class="field">
              <label>Username</label>
              <input type="text" id="user-username" placeholder="johndoe" required />
            </div>
            <div class="field">
              <label>Email</label>
              <input type="email" id="user-email" placeholder="john@example.com" required />
            </div>
          </div>
          <div class="row">
            <div class="field">
              <label>Password</label>
              <input type="password" id="user-password" placeholder="min 8 characters" required />
            </div>
            <div class="field">
              <label>Display Name</label>
              <input type="text" id="user-display-name" placeholder="John Doe" />
            </div>
          </div>
          <div class="field">
            <label>Role</label>
            <select id="user-role">
              <option value="viewer">Viewer</option>
              <option value="deployer">Deployer</option>
              <option value="admin">Admin</option>
            </select>
          </div>
          <div class="form-actions">
            <button type="button" class="btn-cancel" id="btn-cancel">Cancel</button>
            <button type="button" class="btn-save" id="btn-save">Create</button>
          </div>
        </div>
      </app-modal>

      <app-modal id="edit-modal" heading="Edit User">
        <div class="modal-form">
          <div class="row">
            <div class="field">
              <label>Username</label>
              <input type="text" id="edit-username" />
            </div>
            <div class="field">
              <label>Email</label>
              <input type="email" id="edit-email" />
            </div>
          </div>
          <div class="row">
            <div class="field">
              <label>Display Name</label>
              <input type="text" id="edit-display-name" />
            </div>
            <div class="field">
              <label>New Password</label>
              <input type="password" id="edit-password" placeholder="leave blank to keep" />
            </div>
          </div>
          <p class="hint">Leave password blank to keep the current one.</p>
          <div class="form-actions">
            <button type="button" class="btn-cancel" id="edit-cancel">Cancel</button>
            <button type="button" class="btn-save" id="edit-save">Save</button>
          </div>
        </div>
      </app-modal>
    `;

    const table = this.querySelector('#users-table');
    table.columns = [
      { key: 'username', label: 'Username', width: '18%' },
      { key: 'email', label: 'Email', width: '22%' },
      { key: 'display_name', label: 'Name', width: '14%', render: (v) => v || '—' },
      { key: 'role', label: 'Role', width: '11%', render: (v, row) => {
        if (row.is_superuser) return '<app-badge variant="warning">superuser</app-badge>';
        const variant = v === 'admin' ? 'info' : v === 'deployer' ? 'success' : 'neutral';
        return `<app-badge variant="${variant}">${v || 'viewer'}</app-badge>`;
      }},
      { key: 'is_active', label: 'Status', width: '9%', render: (v) =>
        v ? '<app-badge variant="success">active</app-badge>' : '<app-badge variant="neutral">disabled</app-badge>'
      },
      { key: 'created_at', label: 'Created', width: '10%', render: (v) => v ? new Date(v).toLocaleDateString() : '—' },
      { key: 'actions', label: '', width: '16%', render: (_, row) => {
        if (row.is_superuser) return '';
        const esc = (s) => (s || '').replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        const edit = `<button class="action-btn" data-action="edit" data-id="${esc(row.id)}" data-username="${esc(row.username)}" data-email="${esc(row.email)}" data-display-name="${esc(row.display_name || '')}" title="Edit">${icons.edit('', 16)}</button>`;
        const deactivate = row.is_active
          ? `<button class="action-btn action-btn--warn" data-action="deactivate" data-id="${esc(row.id)}" title="Deactivate">${icons.xCircle('', 16)}</button>`
          : `<button class="action-btn" data-action="activate" data-id="${esc(row.id)}" title="Activate">${icons.checkCircle('', 16)}</button>`;
        return `${edit}${deactivate}<button class="action-btn action-btn--danger" data-action="delete" data-id="${esc(row.id)}" data-name="${esc(row.username)}" title="Delete">${icons.trash('', 16)}</button>`;
      }},
    ];

    // Create modal
    const modal = this.querySelector('#user-modal');
    this.querySelector('#btn-add-user').addEventListener('click', () => modal.open());
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());

    const createBtn = this.querySelector('#btn-save');
    createBtn.addEventListener('click', withLoading(createBtn, 'Creating…', async () => {
      const username = this.querySelector('#user-username').value.trim();
      const email = this.querySelector('#user-email').value.trim();
      const password = this.querySelector('#user-password').value;
      const display_name = this.querySelector('#user-display-name').value.trim();
      const role = this.querySelector('#user-role').value;
      if (!username || !email) { showToast('Username and email are required'); return; }
      if (password.length < 8) { showToast('Password must be at least 8 characters'); return; }

      const res = await fetch('/api/users', {
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
    }));

    // Edit modal
    const editModal = this.querySelector('#edit-modal');
    let editingId = null;
    this.querySelector('#edit-cancel').addEventListener('click', () => editModal.close());

    const editSaveBtn = this.querySelector('#edit-save');
    editSaveBtn.addEventListener('click', withLoading(editSaveBtn, 'Saving…', async () => {
      if (!editingId) return;
      const username = this.querySelector('#edit-username').value.trim();
      const email = this.querySelector('#edit-email').value.trim();
      const display_name = this.querySelector('#edit-display-name').value.trim();
      const password = this.querySelector('#edit-password').value;
      if (!username || !email) { showToast('Username and email are required'); return; }
      if (password && password.length < 8) { showToast('Password must be at least 8 characters'); return; }

      const body = { username, email, display_name: display_name || null };
      if (password) body.password = password;

      const res = await fetch(`/api/users/${editingId}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error(await res.text());
      editModal.close();
      table.refresh();
      showToast('User updated');
    }));

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
          const res = await fetch(`/api/users/${id}`, { method: 'DELETE' });
          if (!res.ok) throw new Error(res.statusText);
          table.refresh();
          showToast('User deleted');
        } catch (e) { showToast(`Failed: ${e.message}`); }
      } else if (action === 'deactivate' || action === 'activate') {
        const is_active = action === 'activate';
        try {
          const res = await fetch(`/api/users/${id}`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ is_active }),
          });
          if (!res.ok) throw new Error(res.statusText);
          table.refresh();
          showToast(`User ${is_active ? 'activated' : 'deactivated'}`);
        } catch (e) { showToast(`Failed: ${e.message}`); }
      }
    });
  }
}

customElements.define('users-page', UsersPage);
