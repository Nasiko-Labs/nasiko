import { apiFetch } from '/common/services/api.js';
import { showToast } from '/common/utils/toast.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import '/common/components/smart-table.js';
import { icons } from '/common/utils/icons.js';

import styles from './secrets-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class SecretsPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.innerHTML = `
      <header class="page-head">
        <div>
          <h1 class="title-page">Secrets</h1>
          <p class="page-sub">API credentials stored in this workspace. Router configs and agents reference secrets by name.</p>
        </div>
        <app-button variant="primary" id="btn-add-secret">Add secret</app-button>
      </header>
      <smart-table id="secrets-table" data-fn="fetchSecrets" search search-placeholder="Search secrets..." detail limit="15"></smart-table>
      <div class="note-well">${icons.lock('note-icon', 16)}<span>Keys are write-only. Once saved, a secret can be rotated or deleted but never read back — configs reference it by name.</span></div>
      <app-modal id="secret-modal" heading="Add Secret">
        <div class="form-group">
          <label for="secret-key">Key</label>
          <input type="text" id="secret-key" placeholder="API_KEY" pattern="[A-Z0-9_]+" />
        </div>
        <div class="form-group">
          <label for="secret-value">Value</label>
          <input type="password" id="secret-value" placeholder="sk-..." />
        </div>
        <div class="form-actions">
          <app-button variant="ghost" id="btn-cancel">Cancel</app-button>
          <app-button variant="primary" id="btn-save">Save</app-button>
        </div>
      </app-modal>
    `;

    const table = this.querySelector('#secrets-table');
    table.columns = [
      { key: 'key', label: 'Key', width: '32%', render: (v) => `<span class="cell-key">${v}</span>` },
      { key: 'value', label: 'Value', width: '26%', render: () => '<span class="cell-masked">••••••••</span>' },
      { key: 'created_at', label: 'Created', width: '22%', render: (v) => v ? `<span class="cell-num">${new Date(v).toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' })}</span>` : '—' },
      { key: 'actions', label: '', width: '20%', render: (_, row) => `<button class="delete-btn" data-key="${row.key}">Delete</button>` },
    ];

    const modal = this.querySelector('#secret-modal');
    this.querySelector('#btn-add-secret').addEventListener('click', () => modal.open());
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());

    const saveBtn = this.querySelector('#btn-save');
    saveBtn.addEventListener('click', async () => {
      if (saveBtn.hasAttribute('loading')) return;
      const key = this.querySelector('#secret-key').value.trim();
      const value = this.querySelector('#secret-value').value;
      if (!key || !value) return;
      saveBtn.setAttribute('loading', '');
      try {
        const res = await apiFetch('/secrets', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ key, value }),
        });
        if (!res.ok) throw new Error(await res.text());
        modal.close();
        this.querySelector('#secret-key').value = '';
        this.querySelector('#secret-value').value = '';
        table.refresh();
        showToast('Secret saved');
      } catch (e) {
        showToast(`Failed: ${e.message}`);
      } finally {
        saveBtn.removeAttribute('loading');
      }
    });

    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('.delete-btn');
      if (!btn) return;
      if (!confirm(`Delete secret "${btn.dataset.key}"?`)) return;
      try {
        const res = await apiFetch(`/secrets/${btn.dataset.key}`, { method: 'DELETE' });
        if (!res.ok) throw new Error(res.statusText);
        table.refresh();
      } catch (e) { showToast(`Failed: ${e.message}`); }
    });
  }
}

customElements.define('secrets-page', SecretsPage);
