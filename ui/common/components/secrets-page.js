import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';
import '/common/components/app-modal.js';
import '/common/components/smart-table.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (secrets-page) {
  :scope { display: block; }
  .actions-bar {
    display: flex;
    justify-content: flex-end;
    margin-bottom: var(--space-md);
  }
  .add-btn {
    display: flex;
    align-items: center;
    gap: var(--space-xs);
    padding: var(--space-xs) var(--space-md);
    border: 1px solid var(--color-primary);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--color-primary) 10%, transparent);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    font-weight: 500;
    cursor: pointer;
  }
  .add-btn:hover { background: color-mix(in srgb, var(--color-primary) 20%, transparent); }
  .form-group {
    margin-bottom: var(--space-md);
  }
  .form-group label {
    display: block;
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--color-text-main);
    margin-bottom: var(--space-xs);
  }
  .form-group input {
    width: 100%;
    padding: var(--space-sm) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-base);
    font-family: inherit;
  }
  .form-group input:focus {
    outline: none;
    border-color: var(--color-primary);
    box-shadow: 0 0 0 3px var(--color-primary-ring);
  }
  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-sm);
    margin-top: var(--space-lg);
  }
  .form-actions button {
    padding: var(--space-xs) var(--space-md);
    border-radius: var(--radius-md);
    font-size: var(--font-size-sm);
    font-weight: 500;
    cursor: pointer;
  }
  .btn-save {
    background: var(--color-primary);
    color: white;
    border: none;
  }
  .btn-save:hover { opacity: 0.9; }
  .btn-cancel {
    background: var(--color-bg-surface);
    border: 1px solid var(--color-border);
    color: var(--color-text-main);
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class SecretsPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <div class="actions-bar">
        <button class="add-btn" id="btn-add-secret">${icons.plus('', 16)} Add Secret</button>
      </div>
      <smart-table id="secrets-table" data-fn="fetchSecrets" search search-placeholder="Search secrets..." detail limit="15"></smart-table>
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
          <button class="btn-cancel" id="btn-cancel">Cancel</button>
          <button class="btn-save" id="btn-save">Save</button>
        </div>
      </app-modal>
    `;

    const table = this.querySelector('#secrets-table');
    table.columns = [
      { key: 'key', label: 'Key', width: '30%', render: (v) => `<code style="font-size:var(--font-size-sm);">${v}</code>` },
      { key: 'value', label: 'Value', width: '30%', render: () => '<span style="color:var(--color-text-muted);">••••••••</span>' },
      { key: 'created_at', label: 'Created', width: '20%', render: (v) => v ? new Date(v).toLocaleDateString() : '—' },
      { key: 'actions', label: '', width: '20%', render: (_, row) => `<button class="delete-btn" data-key="${row.key}" style="color:var(--color-error);background:none;border:none;cursor:pointer;font-size:var(--font-size-sm);">Delete</button>` },
    ];

    const modal = this.querySelector('#secret-modal');
    this.querySelector('#btn-add-secret').addEventListener('click', () => modal.open());
    this.querySelector('#btn-cancel').addEventListener('click', () => modal.close());

    const saveBtn = this.querySelector('#btn-save');
    saveBtn.addEventListener('click', withLoading(saveBtn, 'Saving…', async () => {
      const key = this.querySelector('#secret-key').value.trim();
      const value = this.querySelector('#secret-value').value;
      if (!key || !value) return;
      const res = await fetch('/api/secrets', {
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
    }));

    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('.delete-btn');
      if (!btn) return;
      if (!confirm(`Delete secret "${btn.dataset.key}"?`)) return;
      try {
        const res = await fetch(`/api/secrets/${btn.dataset.key}`, { method: 'DELETE' });
        if (!res.ok) throw new Error(res.statusText);
        table.refresh();
      } catch (e) { showToast(`Failed: ${e.message}`); }
    });
  }
}

customElements.define('secrets-page', SecretsPage);
