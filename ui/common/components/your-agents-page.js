import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';
import '/common/components/app-modal.js';
import '/common/components/app-badge.js';

import styles from './your-agents-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { running: 'success', stopped: 'warning', failed: 'error', deploying: 'info', starting: 'info', registered: 'neutral' };

class YourAgentsPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <smart-table
        data-fn="fetchContainers"
        search
        search-placeholder="Search your agents..."
        limit="15"
      ></smart-table>

      <app-modal id="deploy-modal" heading="Deploy Agent">
        <div class="modal-section">
          <h3>Environment Variables</h3>
          <p>These will be injected into the container. Saved to agent secrets for future deploys.</p>
          <div id="env-rows"></div>
          <div style="display:flex;gap:var(--space-sm);margin-top:var(--space-xs);">
            <button class="add-env-btn" id="btn-add-env">${icons.plus('', 14)} Add variable</button>
            <button class="import-btn" id="btn-import-secrets">${icons.key('', 14)} Import from secrets</button>
          </div>
        </div>
        <div class="modal-section" id="secrets-import-section" style="display:none;">
          <h3>Select secrets to import</h3>
          <div class="secret-chips" id="secret-chips"></div>
        </div>
        <div class="form-actions">
          <button class="btn-cancel" id="deploy-cancel">Cancel</button>
          <button class="btn-deploy" id="deploy-confirm">Deploy</button>
        </div>
      </app-modal>
    `;

    const table = this.querySelector('smart-table');
    table.columns = [
      { key: 'name', label: 'Name', width: '20%', render: (v, row) => `<a href="/agent.html?id=${this.#escAttr(row.id)}" style="color:var(--color-primary);text-decoration:none;font-weight:500;">${this.#esc(row.display_name || v)}</a>` },
      { key: 'image', label: 'Image', width: '22%', render: (v) => `<code style="font-size:var(--font-size-xs);background:var(--color-bg-base);padding:2px 6px;border-radius:var(--radius-sm);">${this.#esc(v || '—')}</code>` },
      { key: 'status', label: 'Status', width: '12%', render: (v) => `<app-badge variant="${STATUS_VARIANTS[v] || 'neutral'}">${v}</app-badge>` },
      { key: 'version', label: 'Version', width: '8%', render: (v) => v || '—' },
      { key: 'actions', label: '', width: '20%', render: (_, row) => {
        const esc = (s) => (s || '').replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
        if (row.status === 'running') {
          return `<div style="display:flex;gap:var(--space-xs);">
            <app-button size="xs" data-action="restart" data-name="${esc(row.name)}">Restart</app-button>
            <app-button size="xs" variant="ghost" data-action="stop" data-name="${esc(row.name)}">Stop</app-button>
          </div>`;
        }
        return `<app-button size="xs" variant="primary" data-action="deploy" data-id="${esc(row.id)}" data-name="${esc(row.name)}" data-image="${esc(row.image || '')}">Deploy</app-button>`;
      }},
    ];

    // Deploy modal state
    let deployAgentId = null;
    let deployAgentName = null;
    let deployImage = null;
    let userSecrets = [];
    let selectedSecrets = new Set();

    const modal = this.querySelector('#deploy-modal');
    const envRows = this.querySelector('#env-rows');
    const secretsSection = this.querySelector('#secrets-import-section');
    const secretChips = this.querySelector('#secret-chips');

    const addEnvRow = (key = '', value = '') => {
      const row = document.createElement('div');
      row.className = 'env-row';
      row.innerHTML = `<input type="text" placeholder="KEY" value="${this.#escAttr(key)}" /><input type="text" placeholder="value" value="${this.#escAttr(value)}" /><button class="env-remove">${icons.xCircle('', 16)}</button>`;
      row.querySelector('.env-remove').addEventListener('click', () => row.remove());
      envRows.appendChild(row);
    };

    this.querySelector('#btn-add-env').addEventListener('click', () => addEnvRow());

    this.querySelector('#btn-import-secrets').addEventListener('click', async () => {
      if (secretsSection.style.display !== 'none') {
        secretsSection.style.display = 'none';
        return;
      }
      try {
        const res = await fetch('/api/secrets');
        if (!res.ok) throw new Error();
        userSecrets = await res.json();
      } catch { userSecrets = []; }

      if (!userSecrets.length) {
        showToast('No user secrets found. Add them in Settings → Secrets.');
        return;
      }

      secretChips.innerHTML = userSecrets.map(s =>
        `<span class="secret-chip" data-name="${this.#escAttr(s.name)}">${this.#esc(s.name)}</span>`
      ).join('');
      secretsSection.style.display = '';

      secretChips.querySelectorAll('.secret-chip').forEach(chip => {
        chip.addEventListener('click', () => {
          const name = chip.dataset.name;
          if (selectedSecrets.has(name)) {
            selectedSecrets.delete(name);
            chip.classList.remove('selected');
          } else {
            selectedSecrets.add(name);
            chip.classList.add('selected');
          }
        });
      });
    });

    this.querySelector('#deploy-cancel').addEventListener('click', () => modal.close());

    const deployBtn = this.querySelector('#deploy-confirm');
    deployBtn.addEventListener('click', withLoading(deployBtn, 'Deploying…', async () => {
      // Import selected user secrets into agent secrets
      if (selectedSecrets.size > 0 && deployAgentId) {
        await fetch(`/api/catalog/agents/${deployAgentId}/secrets/import`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ secret_names: [...selectedSecrets] }),
        });
      }

      // Save inline env vars as agent secrets
      const rows = envRows.querySelectorAll('.env-row');
      for (const row of rows) {
        const inputs = row.querySelectorAll('input');
        const key = inputs[0].value.trim();
        const val = inputs[1].value;
        if (!key) continue;
        await fetch(`/api/catalog/agents/${deployAgentId}/secrets`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name: key, value: val }),
        });
      }

      // Deploy container
      const res = await fetch('/api/containers/pull', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          image: deployImage,
          name: deployAgentName,
          agent_id: deployAgentId,
        }),
      });
      if (!res.ok) throw new Error(await res.text());

      modal.close();
      table.refresh();
      showToast(`Deployed ${deployAgentName}`);
    }));

    // Action clicks
    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('[data-action]');
      if (!btn) return;
      const { action } = btn.dataset;

      if (action === 'deploy') {
        deployAgentId = btn.dataset.id;
        deployAgentName = btn.dataset.name;
        deployImage = btn.dataset.image;
        envRows.innerHTML = '';
        selectedSecrets.clear();
        secretsSection.style.display = 'none';

        // Load existing agent secrets to pre-fill
        try {
          const res = await fetch(`/api/catalog/agents/${deployAgentId}/secrets`);
          if (res.ok) {
            const secrets = await res.json();
            if (secrets.length) {
              for (const s of secrets) addEnvRow(s.name, '••••••');
            }
          }
        } catch {}

        modal.setAttribute('heading', `Deploy ${deployAgentName}`);
        modal.open();
      } else if (action === 'restart' || action === 'stop') {
        const name = btn.dataset.name;
        try {
          await fetch(`/api/containers/${encodeURIComponent(name)}/${action}`, { method: 'POST' });
          table.refresh();
        } catch (err) {
          showToast(`Failed to ${action}: ${err.message}`);
        }
      }
    });
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }

  #escAttr(s) {
    return (s || '').replace(/&/g,'&amp;').replace(/"/g,'&quot;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
  }
}

customElements.define('your-agents-page', YourAgentsPage);
