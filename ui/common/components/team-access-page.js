import { showToast } from '/common/utils/toast.js';
import '/common/components/app-module-nav.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';

import styles from './team-access-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const ROLE_ORDER = ['member', 'team_member', 'team_lead', 'department_manager', 'admin'];

/// Roles a caller with `callerRole` may grant — mirrors org_users.rs's
/// `can_manage_target_role`: strictly below the caller's own rank, unless
/// superuser (unrestricted). UX convenience only — the server independently
/// re-validates and is the actual boundary.
function grantableRoles(callerRole, isSuperuser) {
  if (isSuperuser) return ROLE_ORDER;
  const rank = ROLE_ORDER.indexOf(callerRole);
  return ROLE_ORDER.slice(0, Math.max(rank, 0));
}

class TeamAccessPage extends HTMLElement {
  #initialized = false;
  #ctx = null;
  #teams = [];

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#init();
  }

  async #init() {
    try {
      this.#ctx = await window.fetchOrgContext();
    } catch {
      this.innerHTML = `
      <app-module-nav module="org"></app-module-nav>
      <p class="muted">Could not load your org context.</p>`;
      return;
    }

    const canReassignPlacement = this.#ctx.is_superuser || this.#ctx.role === 'admin' || this.#ctx.role === 'department_manager';

    this.innerHTML = `
      <app-module-nav module="org"></app-module-nav>
      <div class="page-head">
        <h1 class="title-page">Team access</h1>
        <span class="scope-line">Viewing as <span class="badge badge--info">${this.#esc(this.#ctx.role)}</span> — showing users in your ${this.#ctx.role === 'department_manager' ? 'department' : 'team'}.</span>
      </div>
      <smart-table id="org-users-table" data-fn="fetchOrgUsers" search search-placeholder="Search..." limit="20"></smart-table>

      <app-modal id="edit-modal" heading="Edit Access">
        <div class="modal-form">
          <div class="field">
            <label>Role</label>
            <select id="edit-role"></select>
          </div>
          <div class="field" id="team-field" ${canReassignPlacement ? '' : 'hidden'}>
            <label>Team</label>
            <select id="edit-team"></select>
          </div>
          <div class="form-actions" data-slot="footer">
            <app-button variant="secondary" id="edit-cancel">Cancel</app-button>
            <app-button variant="primary" id="edit-save">Save</app-button>
          </div>
        </div>
      </app-modal>
    `;

    // Set columns synchronously, right after the <smart-table> is created —
    // it starts its own async refresh() as soon as it's connected (using
    // whatever this.columns is at that instant), so any `await` inserted
    // between the innerHTML assignment and #setupTable() risks that first
    // refresh rendering with the default (raw object-key) columns instead
    // of these, with nothing forcing a second render afterward.
    this.#setupTable();

    // Fetched for every role (not just canReassignPlacement) — team_lead
    // still needs it purely to resolve their own team_id to a name for
    // display; `GET /teams` is itself already scoped (a team_lead sees just
    // their own team, a department_manager their department's teams) so no
    // extra filtering is needed here.
    try {
      this.#teams = (await window.fetchTeams?.('', 1, 100))?.data || [];
    } catch { this.#teams = []; }
    // The initial smart-table refresh() may have already rendered with
    // this.#teams still empty (team-name cells showing "--") if it resolved
    // before this fetch did — force a fresh render now that names are known.
    this.querySelector('#org-users-table')?.refresh();

    this.#bind(canReassignPlacement);
  }

  #setupTable() {
    const table = this.querySelector('#org-users-table');
    table.columns = [
      { key: 'display_name', label: 'User', width: '30%', render: (v, row) => {
        const name = v || row.username;
        const initials = (name || '?').split(/\s+/).map((p) => p[0]).slice(0, 2).join('');
        return `<div class="user-cell">
          <span class="user-avatar">${this.#esc(initials)}</span>
          <span>
            <span class="user-name">${this.#esc(name)}</span><br>
            <span class="user-email">${this.#esc(row.email)}</span>
          </span>
        </div>`;
      }},
      { key: 'role', label: 'Role', width: '18%', render: (v) => `<span class="badge badge--neutral">${this.#esc(v || 'member')}</span>` },
      { key: 'team_id', label: 'Team', width: '20%', render: (v) => this.#teamName(v) },
      { key: 'is_active', label: 'Status', width: '12%', render: (v) =>
        v ? '<span class="badge badge--success">Active</span>'
          : '<span class="badge badge--muted">Disabled</span>' },
      { key: 'actions', label: '', width: '15%', render: (_, row) => {
        if (row.id === this.#ctx.id) return '<span class="muted">you</span>';
        const esc = (s) => (s == null ? '' : String(s)).replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        return `<button class="row-action" data-action="edit" data-id="${esc(row.id)}"` +
          ` data-role="${esc(row.role)}" data-team-id="${esc(row.team_id)}" title="Edit access">Edit</button>`;
      }},
    ];
  }

  #teamName(id) {
    if (!id) return '<span class="muted">--</span>';
    const t = this.#teams.find((x) => x.id === id);
    return t ? `<span class="tag-chip">${this.#esc(t.name)}</span>` : '<span class="muted">--</span>';
  }

  #bind(canReassignPlacement) {
    const table = this.querySelector('#org-users-table');
    const modal = this.querySelector('#edit-modal');
    const roleSelect = this.querySelector('#edit-role');
    const teamSelect = this.querySelector('#edit-team');

    const roles = grantableRoles(this.#ctx.role, this.#ctx.is_superuser);
    roleSelect.innerHTML = roles.map((r) => `<option value="${r}">${r}</option>`).join('');

    if (canReassignPlacement) {
      teamSelect.innerHTML = '<option value="">— No team (unassigned) —</option>'
        + this.#teams.map((t) => `<option value="${t.id}">${this.#esc(t.name)}</option>`).join('');
    }

    this.querySelector('#edit-cancel').addEventListener('click', () => modal.close());

    let editingId = null;
    let originalRole = null;
    let originalTeamId = null;

    const saveBtn = this.querySelector('#edit-save');
    saveBtn.addEventListener('click', async () => {
      if (!editingId) return;
      saveBtn.setAttribute('loading', '');
      try {
        const newRole = roleSelect.value;
        const newTeamId = canReassignPlacement ? teamSelect.value : null;

        if (newRole !== originalRole) {
          const res = await window.updateOrgUserRole(editingId, newRole);
          if (!res.ok) throw new Error(await res.text());
        }
        if (canReassignPlacement && newTeamId !== originalTeamId) {
          const res = newTeamId
            ? await window.updateOrgUserPlacement(editingId, { teamId: newTeamId })
            : await window.updateOrgUserPlacement(editingId, { clear: true });
          if (!res.ok) throw new Error(await res.text());
        }

        modal.close();
        table.refresh();
        showToast('Access updated');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        saveBtn.removeAttribute('loading');
      }
    });

    this.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-action="edit"]');
      if (!btn) return;
      editingId = btn.dataset.id;
      originalRole = btn.dataset.role || 'member';
      originalTeamId = btn.dataset.teamId || '';
      roleSelect.value = roles.includes(originalRole) ? originalRole : roles[0];
      if (canReassignPlacement) teamSelect.value = originalTeamId;
      modal.open();
    });
  }

  #esc(value) {
    if (value == null) return '';
    return String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }
}

customElements.define('team-access-page', TeamAccessPage);
