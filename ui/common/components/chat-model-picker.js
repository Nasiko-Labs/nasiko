import { fetchApi } from '/common/services/api.js';
import { DropdownController } from '/common/components/dropdown-controller.js';
import { showToast } from '/common/utils/toast.js';
import { icons } from '/common/utils/icons.js';

// Compact model switcher that lives inside the chat composer.
//
// It has two modes, because the two chat surfaces resolve their LLM differently:
//
//   agent mode      (agent-id set)  — one agent answers, so pin that agent's model.
//                   GET/PATCH /api/agents/{id}/llm-config  { pinned_model }
//
//   workspace mode  (no agent-id)   — the orchestrator routes each message to an
//                   agent it picks, so there is no single agent to pin. The same
//                   model list is offered; choosing one rewrites the workspace's
//                   default configuration to that provider/model across all three
//                   reasoning tiers (which is how these configs are already used —
//                   "one model to every reasoning level").
//                   GET/PATCH /api/llm-configs, /api/llm-configs/{id}
//
// Both write a *persisted* setting rather than overriding a single message. The LLM
// router resolves provider/model server-side and discards any model the caller sends
// (`llm-router/src/resolver`: `RequestHint` is documented as ignored whenever an
// `llm_config` is present), so a per-message override would be thrown away. These two
// fields are the ones resolution actually honours.

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (chat-model-picker) {
  :scope { display: inline-block; min-width: 0; }

  .trigger {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2xs, 4px);
    max-width: 240px;
    padding: 3px var(--space-xs, 8px);
    border: 1px solid var(--color-border);
    border-radius: var(--r-8, 8px);
    background: transparent;
    color: var(--color-text-muted);
    font-size: var(--font-size-xs, 12px);
    font-family: inherit;
    line-height: 1.5;
    cursor: pointer;
  }
  .trigger:hover:not(:disabled) {
    color: var(--color-text);
    background: var(--bg-surface);
  }
  .trigger:disabled { opacity: .6; cursor: progress; }
  .trigger .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .trigger .chevron { flex: none; display: inline-flex; opacity: .7; }

  .menu {
    position: fixed;
    z-index: 60;
    margin: 0;
    padding: var(--space-2xs, 4px);
    list-style: none;
    max-height: 320px;
    min-width: 260px;
    overflow-y: auto;
    border: 1px solid var(--color-border);
    border-radius: var(--r-8, 8px);
    background: var(--bg-surface);
    box-shadow: var(--shadow-md, 0 8px 24px rgb(0 0 0 / .18));
  }
  .menu.hidden { display: none; }

  .group {
    padding: var(--space-2xs, 4px) var(--space-xs, 8px);
    font-size: var(--font-size-2xs, 11px);
    font-weight: 600;
    letter-spacing: .04em;
    text-transform: uppercase;
    color: var(--color-text-muted);
  }

  .opt {
    padding: 6px var(--space-xs, 8px);
    border-radius: var(--r-6, 6px);
    font-size: var(--font-size-sm, 13px);
    color: var(--color-text);
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* DropdownController owns .selected + aria-selected for keyboard highlight,
     so the *current* choice is marked with its own attribute instead. */
  .opt:hover, .opt.selected { background: var(--bg-surface-hover, rgb(0 0 0 / .06)); }
  .opt[data-current="true"] { font-weight: 600; }
  .opt[data-current="true"]::after { content: " ✓"; }

  .opt.revert { border-bottom: 1px solid var(--color-border); border-radius: 0; margin-bottom: 2px; }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class ChatModelPicker extends HTMLElement {
  #agentId = null;
  #mode = 'agent';       // 'agent' | 'workspace'
  #providers = [];
  #configs = [];
  #pinnedModel = null;
  #config = null;
  #busy = false;
  #dd = null;
  #onDocClick = null;

  connectedCallback() {
    this.#agentId = this.getAttribute('agent-id') || null;
    this.#mode = this.#agentId ? 'agent' : 'workspace';
    this.#renderTrigger('Loading…', true);
    this.#load();
  }

  disconnectedCallback() {
    if (this.#onDocClick) document.removeEventListener('click', this.#onDocClick);
  }

  async #load() {
    if (this.#mode === 'agent') {
      const [configRes, providersRes] = await Promise.all([
        fetchApi(`/agents/${encodeURIComponent(this.#agentId)}/llm-config`).catch((e) => ({ __error: e.message })),
        fetchApi('/llm-router/providers').catch(() => ({ data: [] })),
      ]);
      if (configRes.__error) {
        // Non-owners cannot read or change the config; hide rather than show a
        // control that is guaranteed to fail.
        this.replaceChildren();
        return;
      }
      const payload = configRes?.data ?? configRes;
      this.#pinnedModel = payload?.pinned_model ?? null;
      this.#config = payload?.llm_config || null;
      this.#providers = providersRes?.data ?? [];
    } else {
      const [cfgRes, providersRes] = await Promise.all([
        fetchApi('/llm-configs').catch(() => ({ data: [] })),
        fetchApi('/llm-router/providers').catch(() => ({ data: [] })),
      ]);
      this.#configs = cfgRes?.data ?? (Array.isArray(cfgRes) ? cfgRes : []);
      this.#providers = providersRes?.data ?? [];
      // With no configuration to rewrite there is nothing a choice could change.
      if (!this.#configs.length) {
        this.replaceChildren();
        return;
      }
    }
    this.#render();
  }

  /* ── Rendering ───────────────────────────────────────────────────────── */

  #currentLabel() {
    if (this.#mode === 'workspace') {
      const cfg = this.#targetConfig();
      const model = cfg?.tier1_model || cfg?.model;
      if (!model) return 'Default model';
      const provider = cfg.provider ? `${this.#cap(cfg.provider)} · ` : '';
      return `${provider}${model}`;
    }
    if (this.#pinnedModel) {
      const provider = this.#providerOf(this.#pinnedModel);
      return provider ? `${this.#cap(provider)} · ${this.#pinnedModel}` : this.#pinnedModel;
    }
    if (this.#config) {
      const provider = this.#cap(this.#config.provider || '');
      return provider ? `${provider} · default` : 'Workspace default';
    }
    return 'Default model';
  }

  #renderTrigger(label, disabled) {
    const title = this.#mode === 'workspace'
      ? 'Workspace LLM configuration used by the orchestrator'
      : 'Model used by this agent';
    this.innerHTML = `
      <button type="button" class="trigger" aria-haspopup="listbox" aria-expanded="false"
              ${disabled ? 'disabled' : ''} title="${title}">
        <span class="label">${this.#esc(label)}</span>
        <span class="chevron">${icons.chevronDown('', 12)}</span>
      </button>`;
  }

  #render() {
    this.#renderTrigger(this.#currentLabel(), false);

    const menu = document.createElement('ul');
    menu.className = 'menu hidden';
    menu.setAttribute('role', 'listbox');
    menu.setAttribute('aria-hidden', 'true');
    menu.innerHTML = this.#modelMenuHtml();
    this.appendChild(menu);

    const trigger = this.querySelector('.trigger');
    this.#dd = new DropdownController(menu, trigger, '.opt');
    this.#dd.bindItems(menu.querySelectorAll('.opt').length, () => this.#choose(menu));

    trigger.addEventListener('click', (e) => {
      e.stopPropagation();
      if (this.#dd.isOpen) this.#dd.close(); else this.#dd.open();
    });
    trigger.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown') { e.preventDefault(); this.#dd.open(); this.#dd.navigate(1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); this.#dd.open(); this.#dd.navigate(-1); }
      else if (e.key === 'Enter' && this.#dd.isOpen) { e.preventDefault(); this.#choose(menu); }
      else if (e.key === 'Escape') this.#dd.close();
    });

    this.#onDocClick = (e) => {
      if (this.#dd?.isOpen && !this.contains(e.target)) this.#dd.close();
    };
    document.addEventListener('click', this.#onDocClick);
  }

  /// One menu for both modes: the catalog's providers and models. Agent mode adds
  /// a lead option that clears the pin and hands resolution back to the config.
  #modelMenuHtml() {
    const current = this.#currentModel();

    const revert = this.#mode === 'agent'
      ? `<li class="opt revert" role="option" data-model=""
            data-current="${this.#pinnedModel ? 'false' : 'true'}">${
              this.#config
                ? `Workspace configuration${this.#config.name ? ` (${this.#esc(this.#config.name)})` : ''}`
                : 'Workspace default'
            }</li>`
      : '';

    const groups = this.#providers.map((p) => {
      const opts = (p.models || []).map((m) => `
        <li class="opt" role="option" data-model="${this.#esc(m.model)}"
            data-provider="${this.#esc(p.provider)}"
            data-current="${m.model === current ? 'true' : 'false'}"
            title="${this.#esc(m.model)}">${this.#esc(m.model)}</li>`).join('');
      return opts ? `<li class="group" role="presentation">${this.#esc(this.#cap(p.provider))}</li>${opts}` : '';
    }).join('');

    return revert + groups;
  }

  /// The model currently in effect, whichever mode we are in.
  #currentModel() {
    if (this.#mode === 'agent') return this.#pinnedModel;
    const cfg = this.#targetConfig();
    return cfg?.tier1_model || cfg?.model || null;
  }

  /// The configuration a workspace-mode choice rewrites: the default if one is
  /// marked, otherwise the only one there is.
  #targetConfig() {
    return this.#configs.find((c) => c.is_default)
      ?? (this.#configs.length === 1 ? this.#configs[0] : null);
  }

  /* ── Selection ───────────────────────────────────────────────────────── */

  #choose(menu) {
    const opt = menu.querySelectorAll('.opt')[this.#dd.selIdx];
    if (!opt) return;
    this.#dd.close();

    const model = opt.dataset.model || null;
    if (model === this.#currentModel()) return; // no-op reselect

    if (this.#mode === 'workspace') {
      if (model) this.#setWorkspaceModel(model, opt.dataset.provider || null);
      return;
    }
    this.#setPinnedModel(model);
  }

  async #setPinnedModel(model) {
    await this.#write(
      () => fetchApi(`/agents/${encodeURIComponent(this.#agentId)}/llm-config`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pinned_model: model }),
      }),
      model ? `Switched to ${model}` : 'Reverted to the workspace configuration',
      { model },
    );
  }

  /// Point the workspace configuration at one model. These configs map a single
  /// model to every reasoning tier, so all three move together — matching how the
  /// "Configure router" screen writes them.
  async #setWorkspaceModel(model, provider) {
    const cfg = this.#targetConfig();
    if (!cfg) return;
    const body = { tier1_model: model, tier2_model: model, tier3_model: model };
    if (provider && provider !== cfg.provider) body.provider = provider;
    await this.#write(
      () => fetchApi(`/llm-configs/${encodeURIComponent(cfg.id)}`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      }),
      `Switched to ${model}`,
      { model, provider },
    );
  }

  async #write(request, successMsg, detail) {
    if (this.#busy) return;
    this.#busy = true;
    const trigger = this.querySelector('.trigger');
    if (trigger) trigger.disabled = true;
    try {
      await request();
      showToast(successMsg);
      this.dispatchEvent(new CustomEvent('model-changed', { detail, bubbles: true }));
      await this.#load();
    } catch (e) {
      showToast(`Could not switch: ${e.message}`);
      if (trigger) trigger.disabled = false;
    } finally {
      this.#busy = false;
    }
  }

  /* ── Helpers ─────────────────────────────────────────────────────────── */

  #providerOf(model) {
    for (const p of this.#providers) {
      if ((p.models || []).some((m) => m.model === model)) return p.provider;
    }
    return null;
  }

  #cap(s) {
    return s ? s.charAt(0).toUpperCase() + s.slice(1) : s;
  }

  #esc(s) {
    return String(s ?? '').replace(/[&<>"']/g, (c) => (
      { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
    ));
  }
}

customElements.define('chat-model-picker', ChatModelPicker);
