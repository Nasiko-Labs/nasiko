import { fetchApi } from '/common/services/api.js';
import '/common/components/app-skeleton.js';
import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';

// Self-service LLM routing config for an agent (Phase 2, P2.7). Reads/writes
// GET|PATCH /api/agents/{id}/llm-config and lists the owner's secrets for the key picker.

const PROVIDERS = ['openai', 'anthropic', 'gemini'];
const INBOUND_FORMATS = ['openai', 'anthropic', 'gemini'];

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (agent-llm-config) {
  :scope { display: block; max-width: 560px; }
  .row { margin-bottom: var(--space-md); }
  .row label {
    display: block;
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--color-text-main);
    margin-bottom: var(--space-xs);
  }
  .row .hint {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    margin-top: 2px;
    font-weight: 400;
  }
  /* DS form controls: 32px flat sand wells, hairline only on focus.
     background-color (not the shorthand) so the global select chevron survives. */
  .row input, .row select {
    width: 100%;
    height: var(--control-h-md);
    padding: 0 var(--s-12);
    border: 1px solid transparent;
    border-radius: var(--r-8);
    background-color: var(--bg-input);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    font-family: inherit;
  }
  .row select { padding-right: 30px; }
  .row input:focus, .row select:focus {
    outline: none;
    border-color: var(--border-hover);
    box-shadow: 0 0 0 2px var(--color-primary-ring);
  }
  .grid-2 { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-md); }
  .actions { display: flex; justify-content: flex-end; gap: var(--space-sm); margin-top: var(--space-lg); }
  /* Dark primary per DS (sand-800 fill), not the legacy gold-on-white. */
  .btn-save {
    min-height: var(--control-h-md);
    padding: 0 var(--s-16);
    border: none;
    border-radius: var(--r-8);
    background: light-dark(var(--sand-800), var(--neutral-200));
    color: light-dark(var(--white), var(--neutral-900));
    font-size: var(--font-size-sm);
    font-weight: 500;
    cursor: pointer;
  }
  .btn-save:hover { opacity: 0.9; }
  .btn-save:focus-visible { outline: none; box-shadow: 0 0 0 2px var(--color-primary-ring); }
  .msg { color: var(--color-text-muted); font-style: italic; }
  .msg.error { color: var(--color-error); font-style: normal; }
  .source-note {
    margin-bottom: var(--space-md);
    font-size: var(--font-size-xs);
    font-style: normal;
  }
  .source-note code { font-style: normal; }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class AgentLlmConfig extends HTMLElement {
  #agentId = null;
  // Which `llm_configs` row backs this agent, and how we got it:
  // 'attached' (this agent points at it), 'owner-default' (the owner's default,
  // shared by every unattached agent) or 'none'.
  #configId = null;
  #configSource = 'none';

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
    // Config requires owner access; secrets are the caller's own. Tolerate either failing.
    const [config, secrets] = await Promise.all([
      fetchApi(`/agents/${this.#agentId}/llm-config`).catch((e) => ({ __error: e.message })),
      fetchApi('/secrets').catch(() => []),
    ]);

    if (config.__error) {
      const denied = /not the agent owner|403/i.test(config.__error);
      this.innerHTML = `<p class="msg${denied ? '' : ' error'}">${
        denied ? 'Only the agent owner can manage its LLM config.' : `Failed to load: ${config.__error}`
      }</p>`;
      return;
    }
    // GET /api/secrets answers with the ApiResponse envelope ({data:[…]});
    // tolerate a bare array too so older servers keep working.
    // The llm-config GET is enveloped the same way — reading it off the top
    // level (as this did) yields undefined for every field, which is why the
    // form always came up blank even for an agent with a config attached.
    const payload = config?.data ?? config;
    this.#configId = payload?.llm_config_id ?? null;
    this.#configSource = payload?.source ?? 'none';
    this.#render(payload, Array.isArray(secrets) ? secrets : (secrets?.data ?? []));
  }

  #render(config, secrets) {
    const llm = config.llm_config || {};
    const inboundFormat = config.inbound_format || 'openai';
    const provider = llm.provider || '';
    const model = llm.model || '';
    const temperature = llm.temperature ?? '';
    const maxTokens = llm.max_tokens ?? '';
    const fallbacks = (llm.fallback_models || []).join(', ');
    const secretName = llm.api_key_secret_name || '';

    const opts = (values, selected) =>
      values.map((v) => `<option value="${v}"${v === selected ? ' selected' : ''}>${v}</option>`).join('');
    const secretOpts = [
      `<option value="">— Platform default key —</option>`,
      ...secrets.map((s) => {
        const name = s.name ?? s.key ?? '';
        const safe = this.#esc(name);
        return `<option value="${safe}"${name === secretName ? ' selected' : ''}>${safe}</option>`;
      }),
    ].join('');

    // Be explicit about what a save will touch — "owner-default" means these
    // values come from a config shared with every other unattached agent, and
    // saving will fork a dedicated one rather than edit that.
    const sourceNote = {
      attached: `Editing <code>${this.#esc(llm.name || 'this agent’s config')}</code>, attached to this agent.`,
      'owner-default': `Showing your default config <code>${this.#esc(llm.name || '')}</code>. Saving creates a dedicated config for this agent and leaves the default alone.`,
      none: 'No config yet — saving creates one dedicated to this agent.',
    }[this.#configSource] || '';

    this.innerHTML = `
      ${sourceNote ? `<p class="msg source-note">${sourceNote}</p>` : ''}
      <div class="grid-2">
        <div class="row">
          <label>Provider</label>
          <select id="provider"><option value="">— select —</option>${opts(PROVIDERS, provider)}</select>
          <div class="hint">Where the call is routed (outbound).</div>
        </div>
        <div class="row">
          <label>Model</label>
          <input id="model" type="text" value="${this.#esc(model)}" placeholder="e.g. gpt-4o-mini" />
          <div class="hint">Provider-native model id. The request's model is ignored.</div>
        </div>
      </div>

      <div class="row">
        <label>Inbound SDK format</label>
        <select id="inbound_format">${opts(INBOUND_FORMATS, inboundFormat)}</select>
        <div class="hint">Which SDK the agent's code speaks. Takes effect on the next deploy.</div>
      </div>

      <div class="grid-2">
        <div class="row">
          <label>Temperature</label>
          <input id="temperature" type="number" step="0.1" min="0" max="2" value="${temperature}" placeholder="default" />
        </div>
        <div class="row">
          <label>Max tokens</label>
          <input id="max_tokens" type="number" min="1" value="${maxTokens}" placeholder="default" />
        </div>
      </div>

      <div class="row">
        <label>Fallback models</label>
        <input id="fallback_models" type="text" value="${this.#esc(fallbacks)}" placeholder="provider/model, provider/model" />
        <div class="hint">Comma-separated, tried in order on failure (e.g. <code>openai/gpt-4o-mini</code>).</div>
      </div>

      <div class="row">
        <label>API key secret</label>
        <select id="api_key_secret_name">${secretOpts}</select>
        <div class="hint">A secret you've stored; leave on platform default to use the shared key.</div>
      </div>

      <div class="actions">
        <button class="btn-save" id="btn-save">Save config</button>
      </div>
    `;

    const saveBtn = this.querySelector('#btn-save');
    saveBtn.addEventListener('click', withLoading(saveBtn, 'Saving…', () => this.#save()));
  }

  /// Provider/model/temperature/max_tokens/fallbacks/api-key all live on an
  /// `llm_configs` row — `PATCH /agents/{id}/llm-config` only understands
  /// `llm_config_id`, `inbound_format` and `pinned_model`, so sending them
  /// there (as this used to) was accepted with a 200 and silently discarded.
  ///
  /// So: write the model fields to the backing config, and send only
  /// `inbound_format` to the agent.
  async #save() {
    const val = (id) => this.querySelector(`#${id}`).value.trim();
    const provider = val('provider');
    const model = val('model');
    if (!provider || !model) {
      showToast('Provider and model are required');
      return;
    }
    const num = (id) => {
      const v = val(id);
      return v === '' ? null : Number(v);
    };
    const configBody = {
      provider,
      model,
      temperature: num('temperature'),
      max_tokens: num('max_tokens'),
      fallback_models: val('fallback_models').split(',').map((s) => s.trim()).filter(Boolean),
      api_key_secret_name: val('api_key_secret_name') || null,
    };
    const inboundFormat = val('inbound_format');

    try {
      // Editing the owner's default in place would silently re-point every
      // other agent that falls back to it, so anything not already attached to
      // THIS agent gets its own config instead.
      if (this.#configSource === 'attached' && this.#configId) {
        await this.#patchConfig(this.#configId, configBody);
      } else {
        this.#configId = await this.#createDedicatedConfig(configBody);
        this.#configSource = 'attached';
      }

      await fetchApi(`/agents/${this.#agentId}/llm-config`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          llm_config_id: this.#configId,
          inbound_format: inboundFormat,
        }),
      });
      showToast('LLM config saved');
    } catch (e) {
      showToast(`Failed: ${e.message}`);
    }
  }

  #esc(str) {
    if (str == null) return '';
    return String(str).replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }

  #patchConfig(id, body) {
    return fetchApi(`/llm-configs/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
  }

  /// Creates (or reuses) a config named for this agent and returns its id.
  /// The name is derived from the agent id so the call is idempotent — a retry
  /// after a partial save updates the same row instead of piling up configs.
  async #createDedicatedConfig(body) {
    const name = `agent-${this.#agentId}`;
    try {
      const created = await fetchApi('/llm-configs', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, ...body }),
      });
      const id = (created?.data ?? created)?.id;
      if (!id) throw new Error('server did not return a config id');
      return id;
    } catch (e) {
      // 409 — we already made this agent's config on an earlier save. Find it
      // and update it rather than failing the save.
      if (!/already exists|409/i.test(e.message)) throw e;
      const list = await fetchApi('/llm-configs');
      const rows = Array.isArray(list) ? list : (list?.data ?? []);
      const existing = rows.find((c) => c.name === name);
      if (!existing) throw e;
      await this.#patchConfig(existing.id, body);
      return existing.id;
    }
  }
}

customElements.define('agent-llm-config', AgentLlmConfig);
