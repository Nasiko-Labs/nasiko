import { fetchApi } from '/common/services/api.js';
import '/common/components/app-skeleton.js';
import '/common/components/app-modal.js';
import '/common/components/app-button.js';
import { showToast } from '/common/utils/toast.js';

// Agent LLM routing — summary card + model pin/revert UX.
// Reads GET /api/agents/{id}/llm-config and GET /api/llm-router/providers.
// Writes PATCH /api/agents/{id}/llm-config with { pinned_model }.

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (agent-llm-config) {
  :scope { display: block; max-width: 560px; }

  .subtitle {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin-bottom: var(--space-md);
  }

  /* Summary card */
  .summary-card {
    border: 1px solid var(--color-border);
    border-radius: var(--r-8);
    background: var(--bg-surface);
    padding: var(--space-sm) var(--space-md);
    margin-bottom: var(--space-md);
  }
  .summary-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: var(--space-sm);
    padding: var(--space-xs) 0;
    font-size: var(--font-size-sm);
  }
  .summary-row + .summary-row { border-top: 1px solid var(--color-border); }
  .summary-label { color: var(--color-text-muted); }
  .summary-value {
    font-family: var(--font-mono);
    font-size: var(--font-size-xs);
    color: var(--color-text-main);
    text-align: right;
  }

  /* Action buttons */
  .actions { display: flex; gap: var(--space-sm); flex-wrap: wrap; align-items: center; }
  .btn-override {
    min-height: var(--control-h-md);
    padding: 0 var(--s-16);
    border: 1px solid var(--color-border);
    border-radius: var(--r-8);
    background: transparent;
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }
  .btn-override:hover {
    border-color: var(--border-hover);
    background: var(--bg-surface-hover);
  }
  .btn-override:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--color-primary-ring);
  }
  .btn-revert {
    min-height: var(--control-h-md);
    padding: 0 var(--s-16);
    border: 1px solid var(--color-border);
    border-radius: var(--r-8);
    background: var(--bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }
  .btn-revert:hover {
    border-color: var(--border-hover);
    background: var(--bg-surface-hover);
  }

  /* Modal form fields */
  .modal-desc {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin-bottom: var(--space-md);
  }
  .radio-row {
    display: flex; align-items: center; gap: var(--space-xs);
    margin-bottom: var(--space-sm); font-size: var(--font-size-sm);
  }
  .radio-row input[type="radio"] { accent-color: var(--yellow-600); }
  .radio-row label { margin: 0; font-weight: 400; cursor: pointer; }
  .radio-sub {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    margin: -4px 0 var(--space-sm) 22px;
  }
  .pin-fields { margin-top: var(--space-sm); padding-left: 22px; }
  .field { margin-bottom: var(--space-md); }
  .field label {
    display: block; font-size: var(--font-size-sm); font-weight: 600;
    color: var(--color-text-main); margin-bottom: var(--space-xs);
  }
  .field select {
    width: 100%; height: var(--control-h-md); padding: 0 var(--s-12);
    border: 1px solid transparent; border-radius: var(--r-8);
    background-color: var(--bg-input); color: var(--color-text-main);
    font-size: var(--font-size-sm); font-family: inherit; padding-right: 30px;
  }
  .field select:focus {
    outline: none; border-color: var(--border-hover);
    box-shadow: 0 0 0 2px var(--color-primary-ring);
  }

  .msg { color: var(--color-text-muted); font-style: italic; }
  .msg.error { color: var(--color-error); font-style: normal; }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class AgentLlmConfig extends HTMLElement {
  #agentId = null;
  #configId = null;   // attached llm_config_id (or null)
  #config = null;     // resolved llm_config object (or null)
  #configSource = 'none'; // 'attached' | 'owner-default' | 'none'
  #pinnedModel = null;
  #providers = [];
  #configs = [];      // user's reusable configs from GET /api/llm-configs
  #busy = false;

  connectedCallback() {
    this.#agentId = this.getAttribute('agent-id');
    if (!this.#agentId) {
      this.innerHTML = '<p class="msg error">No agent ID.</p>';
      return;
    }
    this.innerHTML = '<app-skeleton lines="3"></app-skeleton>';
    this.#load();
  }

  async #load() {
    const [configRes, providersRes, configsRes] = await Promise.all([
      fetchApi(`/agents/${this.#agentId}/llm-config`).catch((e) => ({ __error: e.message })),
      fetchApi('/llm-router/providers').catch(() => ({ data: [] })),
      fetchApi('/llm-configs').catch(() => ({ data: [] })),
    ]);

    if (configRes.__error) {
      const denied = /not the agent owner|403/i.test(configRes.__error);
      this.innerHTML = `<p class="msg${denied ? '' : ' error'}">${
        denied ? 'Only the agent owner can manage its LLM config.' : `Failed to load: ${configRes.__error}`
      }</p>`;
      return;
    }

    const payload = configRes?.data ?? configRes;
    this.#configId = payload?.llm_config_id ?? null;
    this.#config = payload?.llm_config || null;
    this.#configSource = payload?.source ?? 'none';
    this.#pinnedModel = payload?.pinned_model ?? null;
    this.#providers = providersRes?.data ?? [];
    this.#configs = configsRes?.data ?? (Array.isArray(configsRes) ? configsRes : []);
    this.#render();
  }

  #render() {
    const pinned = this.#pinnedModel;
    const cfg = this.#config;
    const pinnedProvider = pinned ? this.#providerOf(pinned) : null;

    // Subtitle line
    let subtitle;
    if (pinned) {
      const provLabel = pinnedProvider ? `${this.#cap(pinnedProvider)} \u00b7 ` : '';
      subtitle = `Uses a pinned model \u00b7 ${provLabel}${this.#esc(pinned)}`;
    } else if (cfg) {
      subtitle = `Uses the workspace configuration \u00b7 ${this.#esc(this.#cap(cfg.provider || ''))}`;
    } else {
      subtitle = 'No configuration resolved for this agent';
    }

    // Summary card rows
    let cardHtml;
    if (pinned) {
      cardHtml = this.#summaryCard([
        ['Current configuration', 'Pinned model'],
        ...(pinnedProvider ? [['Provider', this.#cap(pinnedProvider)]] : []),
        ['Model', pinned],
      ]);
    } else if (cfg) {
      cardHtml = this.#summaryCard([
        ['Current configuration', cfg.name || '—'],
        ['Provider', this.#cap(cfg.provider || '')],
        ['Advanced reasoning', cfg.tier1_model || '—'],
        ['Balanced', cfg.tier2_model || '—'],
        ['Fast responses', cfg.tier3_model || '—'],
      ]);
    } else {
      cardHtml = '';
    }

    this.innerHTML = `
      <p class="subtitle">${subtitle}</p>
      ${cardHtml}
      <div class="actions">
        ${pinned ? `<button class="btn-revert" data-action="revert" type="button">Revert to default</button>` : ''}
        <button class="btn-override" data-action="override" type="button">${pinned ? 'Change model' : 'Override model'}</button>
      </div>
    `;

    this.querySelector('[data-action="override"]')?.addEventListener('click', () => this.#openOverrideModal());
    this.querySelector('[data-action="revert"]')?.addEventListener('click', () => this.#revert());
  }

  #summaryCard(rows) {
    return `<div class="summary-card">${rows.map(([label, value]) => `
      <div class="summary-row">
        <span class="summary-label">${this.#esc(label)}</span>
        <span class="summary-value">${this.#esc(value)}</span>
      </div>`).join('')}</div>`;
  }

  /* ── Override modal ──────────────────────────────────────────────────── */

  #openOverrideModal() {
    // Draft state — only committed on Save.
    let mode = this.#pinnedModel ? 'pin' : 'config'; // 'config' | 'pin'
    let draftConfigId = this.#configId || '';          // '' = owner default
    let draftProvider = this.#pinnedModel ? this.#providerOf(this.#pinnedModel) : null;
    let draftModel = this.#pinnedModel || null;

    // Remove any existing modal
    this.querySelector('app-modal')?.remove();

    const modal = document.createElement('app-modal');
    modal.setAttribute('heading', 'Override model');

    const body = document.createElement('div');
    const footer = document.createElement('div');
    footer.dataset.slot = 'footer';

    const renderBody = () => {
      const modelsHtml = draftProvider
        ? this.#providers.find((p) => p.provider === draftProvider)?.models
            ?.map((m) => `<option value="${this.#esc(m.model)}" ${m.model === draftModel ? 'selected' : ''}>${this.#esc(m.model)}</option>`)
            .join('') || ''
        : '';

      const defaultCfg = this.#configs.find((c) => c.is_default);
      const configOptions = this.#configs.map((c) => {
        const label = c.is_default ? `${this.#esc(c.name)} (default)` : this.#esc(c.name);
        const selected = c.id === draftConfigId ? 'selected' : '';
        return `<option value="${c.id}" ${selected}>${label}</option>`;
      }).join('');

      body.innerHTML = `
        <p class="modal-desc">Choose how this agent selects an LLM.</p>
        <div class="radio-row">
          <input type="radio" id="pin-mode-config" name="pin-mode" value="config" ${mode === 'config' ? 'checked' : ''} />
          <label for="pin-mode-config">Use workspace configuration</label>
        </div>
        ${mode === 'config' ? `
          <div class="pin-fields">
            <div class="field">
              <label for="pick-config">Configuration</label>
              <select id="pick-config">
                <option value="" ${!draftConfigId ? 'selected' : ''}>Workspace default${defaultCfg ? ` (${this.#esc(defaultCfg.name)})` : ''}</option>
                ${configOptions}
              </select>
            </div>
          </div>` : ''}
        <div class="radio-row">
          <input type="radio" id="pin-mode-pin" name="pin-mode" value="pin" ${mode === 'pin' ? 'checked' : ''} />
          <label for="pin-mode-pin">Pin a model</label>
        </div>
        ${mode === 'pin' ? `
          <div class="pin-fields">
            <div class="field">
              <label for="pin-provider">Provider</label>
              <select id="pin-provider">
                <option value="" disabled ${draftProvider ? '' : 'selected'}>Choose provider</option>
                ${this.#providers.map((p) => `
                  <option value="${this.#esc(p.provider)}" ${p.provider === draftProvider ? 'selected' : ''}>
                    ${this.#esc(this.#cap(p.provider))}
                  </option>`).join('')}
              </select>
            </div>
            ${draftProvider ? `
            <div class="field">
              <label for="pin-model">Model</label>
              <select id="pin-model">
                <option value="" disabled ${draftModel ? '' : 'selected'}>Choose model</option>
                ${modelsHtml}
              </select>
            </div>` : ''}
          </div>` : ''}
      `;

      // Wire radio toggles
      body.querySelectorAll('input[name="pin-mode"]').forEach((r) => {
        r.addEventListener('change', () => {
          mode = r.value;
          renderBody();
        });
      });

      // Wire config picker
      body.querySelector('#pick-config')?.addEventListener('change', (e) => {
        draftConfigId = e.target.value;
      });

      // Wire provider change
      body.querySelector('#pin-provider')?.addEventListener('change', (e) => {
        draftProvider = e.target.value;
        draftModel = null;
        renderBody();
      });

      // Wire model change
      body.querySelector('#pin-model')?.addEventListener('change', (e) => {
        draftModel = e.target.value;
      });
    };

    footer.innerHTML = `
      <app-button variant="secondary" data-action="modal-cancel">Cancel</app-button>
      <app-button variant="primary" data-action="modal-save">Save changes</app-button>
    `;

    renderBody();
    modal.appendChild(body);
    modal.appendChild(footer);
    this.appendChild(modal);

    // Wire footer buttons
    footer.querySelector('[data-action="modal-cancel"]').addEventListener('click', () => modal.close());
    footer.querySelector('[data-action="modal-save"]').addEventListener('click', async () => {
      if (mode === 'pin' && (!draftProvider || !draftModel)) {
        showToast('Choose a provider and model');
        return;
      }
      modal.close();
      if (mode === 'pin') {
        await this.#setPinnedModel(draftModel);
      } else {
        await this.#attachConfig(draftConfigId || null);
      }
    });

    // Open after next frame so the dialog element is in the DOM
    requestAnimationFrame(() => modal.open());
  }

  /* ── API calls ───────────────────────────────────────────────────────── */

  async #setPinnedModel(model) {
    if (this.#busy) return;
    this.#busy = true;
    try {
      await fetchApi(`/agents/${this.#agentId}/llm-config`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pinned_model: model }),
      });
      showToast(model ? `Pinned ${model}` : 'Reverted to the workspace configuration');
      // Reload fresh state
      await this.#load();
    } catch (e) {
      showToast(`Failed: ${e.message}`);
    } finally {
      this.#busy = false;
    }
  }

  async #revert() {
    await this.#setPinnedModel(null);
  }

  async #attachConfig(configId) {
    if (this.#busy) return;
    this.#busy = true;
    try {
      await fetchApi(`/agents/${this.#agentId}/llm-config`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ llm_config_id: configId }),
      });
      const cfg = configId ? this.#configs.find((c) => c.id === configId) : null;
      showToast(cfg ? `Attached "${cfg.name}"` : 'Using workspace default');
      await this.#load();
    } catch (e) {
      showToast(`Failed: ${e.message}`);
    } finally {
      this.#busy = false;
    }
  }

  /* ── Helpers ─────────────────────────────────────────────────────────── */

  /// Find which provider a model belongs to by scanning the catalog.
  #providerOf(model) {
    if (!model) return null;
    for (const p of this.#providers) {
      if (p.models?.some((m) => m.model === model)) return p.provider;
    }
    return null;
  }

  #cap(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }

  #esc(str) {
    if (str == null) return '';
    return String(str).replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('agent-llm-config', AgentLlmConfig);