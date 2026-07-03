import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import '/common/components/smart-table.js';

import styles from './teams-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class TeamsPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <div class="teams-stats" id="teams-stats"></div>
      <div class="actions-bar">
        <app-button variant="primary" size="sm" id="btn-create-team">${icons.plus('', 16)} Create Team</app-button>
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
              <option value="">-- None --</option>
            </select>
          </div>
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
      el.innerHTML = `<span class="count">${stats.total}</span> team${stats.total !== 1 ? 's' : ''}, <span class="count">${stats.empty_teams}</span> without members`;
    } catch {
      el.innerHTML = '';
    }
  }

  #setupTable() {
    const table = this.querySelector('#teams-table');
    table.columns = [
      { key: 'name', label: 'Name', width: '30%', render: (v) => `<strong>${v || ''}</strong>` },
      { key: 'department', label: 'Department', width: '25%', render: (v) => v ? v : '--' },
      { key: 'members_count', label: 'Members', width: '20%', render: (v) => v != null ? v : '0' },
      { key: 'agents_count', label: 'Agents', width: '25%', render: (v) => v != null ? v : '0' },
    ];
  }

  #setupModal() {
    const modal = this.querySelector('#create-team-modal');
    const table = this.querySelector('#teams-table');

    this.querySelector('#btn-create-team').addEventListener('click', async () => {
      await this.#loadDepartments();
      modal.open();
    });

    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());

    const saveBtn = this.querySelector('#btn-save');
    saveBtn.addEventListener('click', async () => {
      saveBtn.setAttribute('loading', '');
      try {
        const name = this.querySelector('#team-name').value.trim();
        const department_id = this.querySelector('#team-department').value || null;

        if (!name) {
          showToast('Team name is required');
          return;
        }

        const res = await fetch('/api/teams', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name, department_id }),
        });

        if (!res.ok) throw new Error(await res.text());

        modal.close();
        this.querySelector('#team-name').value = '';
        this.querySelector('#team-department').value = '';
        table.refresh();
        this.#loadStats();
        showToast('Team created');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        saveBtn.removeAttribute('loading');
      }
    });
  }

  async #loadDepartments() {
    const select = this.querySelector('#team-department');
    try {
      const departments = await window.fetchDepartmentList();
      const current = select.value;
      select.innerHTML = '<option value="">-- None --</option>';
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
