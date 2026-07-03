import { icons } from '/common/utils/icons.js';
import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';
import '/common/components/app-modal.js';
import '/common/components/app-badge.js';

import styles from './your-agents-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class YourAgentsPage extends HTMLElement {
  #agents = [];

  connectedCallback() {
    this.innerHTML = `
      <div class="page-header">
        <div class="page-header-top">
          <div>
            <h1 class="page-title">Your Agents</h1>
            <p class="page-desc">Deployed agent containers you manage.</p>
          </div>
        </div>
        <div class="stats-bar" id="stats-bar"></div>
      </div>
      <div class="toolbar">
        <div class="search-wrap">
          <span class="search-icon">${icons.search('', 18)}</span>
          <input type="search" id="search-input" placeholder="Search agents..." />
        </div>
      </div>
      <div class="agents-grid" id="agents-grid">
        ${this.#skeletonCards()}
      </div>

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

    this.querySelector('#search-input').addEventListener('input', () => this.#renderGrid());
    this.#setupModal();
    this.#load();
  }

  async #load() {
    const result = await window.fetchContainers('', 1, 100);
    this.#agents = result.data || [];
    this.#renderStats();
    this.#renderGrid();
  }

  #renderStats() {
    const running = this.#agents.filter(a => a.status === 'running').length;
    const stopped = this.#agents.filter(a => a.status === 'stopped').length;
    const errored = this.#agents.filter(a => a.status === 'error' || a.status === 'failed').length;
    this.querySelector('#stats-bar').innerHTML = `
      <div class="stat-chip"><span class="stat-dot stat-dot--running"></span>${running} running</div>
      <div class="stat-chip"><span class="stat-dot stat-dot--stopped"></span>${stopped} stopped</div>
      ${errored ? `<div class="stat-chip"><span class="stat-dot stat-dot--error"></span>${errored} error</div>` : ''}
      <div class="stat-chip stat-chip--total">${this.#agents.length} total</div>
    `;
  }

  #skeletonCards() {
    return Array.from({ length: 4 }, () => `
      <div class="agent-card">
        <div style="width:50%;height:1.1em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
        <div style="width:80%;height:0.8em;background:var(--color-border);border-radius:var(--radius-sm);margin-top:var(--space-sm);"></div>
        <div style="width:40%;height:0.8em;background:var(--color-border);border-radius:var(--radius-sm);margin-top:var(--space-xs);"></div>
      </div>
    `).join('');
  }

  #renderGrid() {
    const q = (this.querySelector('#search-input')?.value || '').toLowerCase();
    let filtered = this.#agents;
    if (q) {
      filtered = filtered.filter(a =>
        (a.display_name || a.name || '').toLowerCase().includes(q) ||
        (a.image || '').toLowerCase().includes(q)
      );
    }

    const grid = this.querySelector('#agents-grid');
    if (!filtered.length) {
      grid.innerHTML = '<div class="empty-state">No agents found.</div>';
      return;
    }

    grid.innerHTML = filtered.map(a => {
      const name = a.display_name || a.name;
      const isRunning = a.status === 'running';
      const isError = a.status === 'error' || a.status === 'failed';

      return `
        <div class="agent-card${isError ? ' agent-card--error' : ''}">
          <div class="agent-card-header">
            <div class="agent-card-icon">${icons.cube('', 20)}</div>
            <div class="agent-card-status">
              <span class="status-dot status-dot--${a.status}"></span>
              <span class="status-label">${a.status}</span>
            </div>
          </div>
          <a class="agent-card-name" href="/agent-card.html?id=${this.#escAttr(a.id)}">${this.#esc(name)}</a>
          <div class="agent-card-meta">
            <code class="agent-card-image">${this.#esc(a.image || '—')}</code>
            ${a.version ? `<span class="agent-card-version">v${this.#esc(a.version)}</span>` : ''}
          </div>
          <div class="agent-card-actions">
            ${isRunning ? `
              <button class="card-action-btn" data-action="restart" data-name="${this.#escAttr(a.name)}" title="Restart">${icons.refresh('', 14)} Restart</button>
              <button class="card-action-btn" data-action="stop" data-name="${this.#escAttr(a.name)}" title="Stop">${icons.square('', 12)} Stop</button>
            ` : `
              <button class="card-action-btn card-action-btn--primary" data-action="deploy" data-id="${this.#escAttr(a.id)}" data-name="${this.#escAttr(a.name)}" data-image="${this.#escAttr(a.image || '')}">Deploy</button>
            `}
            <button class="card-action-btn card-action-btn--danger" data-action="delete" data-id="${this.#escAttr(a.id)}" data-name="${this.#escAttr(a.name)}" title="Delete">${icons.trash('', 14)}</button>
          </div>
        </div>
      `;
    }).join('');
  }

  #setupModal() {
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
      if (selectedSecrets.size > 0 && deployAgentId) {
        await fetch(`/api/catalog/agents/${deployAgentId}/secrets/import`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ secret_names: [...selectedSecrets] }),
        });
      }

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
      this.#load();
      showToast(`Deployed ${deployAgentName}`);
    }));

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
          this.#load();
          showToast(`${action === 'restart' ? 'Restarted' : 'Stopped'} ${name}`);
        } catch (err) {
          showToast(`Failed to ${action}: ${err.message}`);
        }
      } else if (action === 'delete') {
        const name = btn.dataset.name;
        const id = btn.dataset.id;
        if (!confirm(`Delete agent "${name}"? This will stop the container and remove it from the registry.`)) return;
        try {
          const res = await fetch(`/api/catalog/agents/${encodeURIComponent(id)}`, { method: 'DELETE' });
          if (!res.ok) throw new Error(await res.text());
          this.#load();
          showToast(`Deleted ${name}`);
        } catch (err) {
          showToast(`Failed to delete: ${err.message}`);
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
    return (s || '').replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }
}

customElements.define('your-agents-page', YourAgentsPage);
