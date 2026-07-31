/**
 * LLM router — routing configs (per-reasoning-tier model presets) and the
 * provider/model catalog.
 *
 * @element llm-router-page
 * @note Data sources (see /api/docs):
 *       `window.fetchLlmConfigs()`      → GET  /api/llm-configs
 *       `window.createLlmConfig(body)`  → POST /api/llm-configs
 *       `window.deleteLlmConfig(id)`    → DELETE /api/llm-configs/{id}
 *       `window.setDefaultLlmConfig(id)`→ POST /api/llm-configs/{id}/default
 *       `window.fetchLlmProviders()`    → GET  /api/llm-router/providers
 *       `window.fetchSecretsList()`         → GET  /api/secrets
 */
import styles from './llm-router-page.css' with { type: 'css' };
import { icons } from '../utils/icons.js';
import { showToast } from '../utils/toast.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TIERS = [
  { key: 'tier1_model', label: 'Advanced reasoning', hint: 'For coding, planning, and complex analysis.' },
  { key: 'tier2_model', label: 'Balanced', hint: 'For most everyday requests.' },
  { key: 'tier3_model', label: 'Light', hint: 'For quick, simple lookups and formatting.' },
];

class LlmRouterPage extends HTMLElement {
  #initialized = false;
  #configs = [];
  #providers = [];
  #secrets = [];
  #view = 'list'; // 'list' | 'form'

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.addEventListener('click', (e) => this.#onClick(e));
    this.addEventListener('change', (e) => this.#onChange(e));
    this.#load();
  }

  async #load() {
    this.innerHTML = '<h1 class="page-title">LLM router</h1><p class="group-sub">Loading…</p>';
    try {
      const [configs, providers, secrets] = await Promise.all([
        window.fetchLlmConfigs(),
        window.fetchLlmProviders(),
        window.fetchSecretsList(),
      ]);
      this.#configs = configs?.data ?? [];
      this.#providers = providers?.data ?? [];
      this.#secrets = Array.isArray(secrets) ? secrets : secrets?.data ?? [];
    } catch (e) {
      console.error('LLM router load failed:', e);
      this.innerHTML = '<h1 class="page-title">LLM router</h1><p class="form-error">Failed to load router configuration</p>';
      return;
    }
    this.#view = 'list';
    this.#render();
  }

  #render() {
    this.innerHTML = this.#view === 'form' ? this.#formHtml() : this.#listHtml();
  }

  /* ── List view ─────────────────────────────────────────────────────────── */

  #listHtml() {
    return `
      <h1 class="page-title">LLM router</h1>
      ${this.#configs.length ? this.#configCardsHtml() : this.#emptyHtml()}
      <hr class="divider" />
      <h2 class="section-label">Available providers</h2>
      <div class="provider-grid">
        ${this.#providers.map((p) => this.#providerCardHtml(p)).join('')}
      </div>
    `;
  }

  #emptyHtml() {
    return `
      <div class="empty-configs">
        <div class="empty-title">No configs yet!</div>
        <p class="empty-sub">Connect a provider to start configuring</p>
        <button class="btn-dark" data-action="new-config" type="button">Setup new config</button>
      </div>
    `;
  }

  #configCardsHtml() {
    return `
      <div class="config-list">
        ${this.#configs.map((c) => `
          <div class="config-card">
            <div class="config-head">
              <div>
                <div class="config-name">${this.#esc(c.name)}</div>
                <div class="config-provider">${this.#esc(c.provider)}</div>
              </div>
              ${c.is_default ? '<span class="badge-default">Default</span>' : ''}
            </div>
            <div class="tier-rows">
              ${TIERS.map((t) => `
                <div class="tier-row">
                  <span class="tier-label">${t.label}</span>
                  <span class="tier-model">${this.#esc(c[t.key] || c.model || '—')}</span>
                </div>`).join('')}
            </div>
            <div class="config-actions">
              ${c.is_default ? '' : `<button class="link-btn" data-action="set-default" data-id="${c.id}" type="button">Make default</button>`}
              <button class="link-btn is-danger" data-action="delete-config" data-id="${c.id}" type="button">Delete</button>
            </div>
          </div>`).join('')}
      </div>
      <button class="btn-dark" data-action="new-config" type="button">Setup new config</button>
    `;
  }

  #providerCardHtml(p) {
    const count = p.models?.length ?? 0;
    return `
      <div class="provider-card" data-action="new-config" data-provider="${this.#esc(p.provider)}"
        role="button" tabindex="0">
        <div class="provider-head">
          <span class="provider-glyph">${this.#esc((p.provider || '?')[0])}</span>
          <span class="provider-name">${this.#esc(p.provider)}</span>
          <span class="provider-add">+</span>
        </div>
        <div class="provider-chips">
          <span class="chip-outline">REQUIRES API KEY</span>
          <span class="chip-outline">+${count}</span>
        </div>
      </div>
    `;
  }

  /* ── Configure form view ───────────────────────────────────────────────── */

  #formHtml(provider = '') {
    return `
      <div class="form-head">
        <button class="back-btn" data-action="back" type="button" aria-label="Back">${icons.arrowLeft('', 16)}</button>
        <h1 class="form-title">Configure router</h1>
      </div>
      <form class="config-form" id="config-form">
        <div class="field">
          <label for="cfg-name">Settings name</label>
          <input type="text" id="cfg-name" name="name" placeholder="Enter name" required />
        </div>
        <div class="field">
          <label for="cfg-provider">Provider</label>
          <select id="cfg-provider" name="provider" required>
            <option value="" disabled ${provider ? '' : 'selected'}>Choose Provider</option>
            ${this.#providers.map((p) => `
              <option value="${this.#esc(p.provider)}" ${p.provider === provider ? 'selected' : ''}>
                ${this.#esc(this.#cap(p.provider))}
              </option>`).join('')}
          </select>
        </div>
        <div>
          <h3 class="group-title">Connect provider</h3>
          <p class="group-sub">Connect provider by selecting an existing secret or adding a new one.</p>
          <div class="radio-row">
            <input type="radio" id="secret-saved" name="secret-mode" value="saved" checked />
            <label for="secret-saved">Use saved secret</label>
          </div>
          <div class="radio-row">
            <input type="radio" id="secret-new" name="secret-mode" value="new" />
            <label for="secret-new">Add new secret</label>
          </div>
          <div class="field" id="saved-secret-field">
            <label for="cfg-secret">Use saved secret</label>
            <select id="cfg-secret" name="api_key_secret_name">
              <option value="" disabled selected>Find secrets</option>
              ${this.#secrets.map((s) => `<option value="${this.#esc(s.name)}">${this.#esc(s.name)}</option>`).join('')}
            </select>
            <div class="hint">Select a secret already stored in your workspace.</div>
          </div>
          <div class="field" id="new-secret-field" hidden>
            <label for="cfg-secret-name">Secret name</label>
            <input type="text" id="cfg-secret-name" name="new_secret_name" placeholder="e.g. OPENAI_API_KEY" />
            <label for="cfg-secret-value" style="margin-top: var(--space-sm)">Secret value</label>
            <input type="password" id="cfg-secret-value" name="secret_value" placeholder="Paste the API key" autocomplete="off" />
            <div class="hint">Stored encrypted; only used to call the provider.</div>
          </div>
        </div>
        <div>
          <h3 class="group-title">Reasoning levels</h3>
          <p class="group-sub">Assign a model for each reasoning level. The router automatically selects the appropriate model based on the request.</p>
          ${TIERS.map((t) => `
            <div class="field" style="margin-bottom: var(--space-md)">
              <label for="cfg-${t.key}">${t.label}</label>
              <select id="cfg-${t.key}" name="${t.key}" data-tier-select ${provider ? '' : 'disabled'}>
                <option value="" selected>Choose model</option>
                ${this.#modelOptions(provider)}
              </select>
              <div class="hint">${t.hint}</div>
            </div>`).join('')}
        </div>
        <div class="checkbox-row">
          <input type="checkbox" id="cfg-default" name="is_default" ${this.#configs.length ? '' : 'checked'} />
          <label for="cfg-default">Make this the default routing config</label>
        </div>
        <div class="form-error" id="form-error" hidden></div>
        <div class="form-actions">
          <button class="btn-dark" type="submit">Save config</button>
          <button class="link-btn" data-action="back" type="button">Cancel</button>
        </div>
      </form>
    `;
  }

  #modelOptions(provider) {
    const entry = this.#providers.find((p) => p.provider === provider);
    return (entry?.models ?? [])
      .map((m) => `<option value="${this.#esc(m.model)}">${this.#esc(m.model)}</option>`)
      .join('');
  }

  /* ── Events ────────────────────────────────────────────────────────────── */

  #onClick(e) {
    const el = e.target.closest('[data-action]');
    if (!el) return;
    const action = el.dataset.action;
    if (action === 'new-config') {
      this.#view = 'form';
      this.innerHTML = this.#formHtml(el.dataset.provider || '');
      this.querySelector('#config-form').addEventListener('submit', (ev) => this.#save(ev));
    } else if (action === 'back') {
      this.#view = 'list';
      this.#render();
    } else if (action === 'delete-config') {
      this.#deleteConfig(el.dataset.id);
    } else if (action === 'set-default') {
      this.#setDefault(el.dataset.id);
    }
  }

  #onChange(e) {
    if (e.target.name === 'secret-mode') {
      const useNew = e.target.value === 'new';
      this.querySelector('#saved-secret-field').hidden = useNew;
      this.querySelector('#new-secret-field').hidden = !useNew;
    } else if (e.target.id === 'cfg-provider') {
      const options = this.#modelOptions(e.target.value);
      this.querySelectorAll('[data-tier-select]').forEach((sel) => {
        sel.disabled = false;
        sel.innerHTML = `<option value="" selected>Choose model</option>${options}`;
      });
    }
  }

  async #save(e) {
    e.preventDefault();
    const form = e.target;
    const useNew = form.querySelector('#secret-new').checked;
    const body = {
      name: form.querySelector('#cfg-name').value.trim(),
      provider: form.querySelector('#cfg-provider').value,
      tier1_model: form.querySelector('#cfg-tier1_model').value || null,
      tier2_model: form.querySelector('#cfg-tier2_model').value || null,
      tier3_model: form.querySelector('#cfg-tier3_model').value || null,
      is_default: form.querySelector('#cfg-default').checked,
      api_key_secret_name: useNew
        ? form.querySelector('#cfg-secret-name').value.trim() || null
        : form.querySelector('#cfg-secret').value || null,
      secret_value: useNew ? form.querySelector('#cfg-secret-value').value || null : null,
    };
    const errEl = this.querySelector('#form-error');
    if (!body.tier1_model && !body.tier2_model && !body.tier3_model) {
      errEl.textContent = 'Choose a model for at least one reasoning level.';
      errEl.hidden = false;
      return;
    }
    try {
      await window.createLlmConfig(body);
    } catch (err) {
      errEl.textContent = err?.message || 'Failed to save config';
      errEl.hidden = false;
      return;
    }
    showToast('Router config saved');
    this.#load();
  }

  async #deleteConfig(id) {
    if (!confirm('Delete this routing config?')) return;
    try {
      await window.deleteLlmConfig(id);
    } catch (err) {
      showToast(err?.message || 'Failed to delete config');
      return;
    }
    this.#load();
  }

  async #setDefault(id) {
    try {
      await window.setDefaultLlmConfig(id);
    } catch (err) {
      showToast(err?.message || 'Failed to set default');
      return;
    }
    this.#load();
  }

  #cap(s) {
    return s ? s[0].toUpperCase() + s.slice(1) : s;
  }

  #esc(str) {
    if (str == null) return '';
    return String(str).replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('llm-router-page', LlmRouterPage);
