import { fetchApi } from '/common/services/api.js';
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
  .row input, .row select {
    width: 100%;
    padding: var(--space-sm) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-base);
    font-family: inherit;
  }
  .row input:focus, .row select:focus {
    outline: none;
    border-color: var(--color-primary);
    box-shadow: 0 0 0 3px var(--color-primary-ring);
  }
  .grid-2 { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-md); }
  .actions { display: flex; justify-content: flex-end; gap: var(--space-sm); margin-top: var(--space-lg); }
  .btn-save { background: var(--color-primary); color: #fff; border: none; padding: var(--space-xs) var(--space-md); border-radius: var(--radius-md); font-size: var(--font-size-sm); font-weight: 500; cursor: pointer; }
  .msg { color: var(--color-text-muted); font-style: italic; }
  .msg.error { color: var(--color-error); font-style: normal; }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class AgentLlmConfig extends HTMLElement {
  #agentId = null;

  connectedCallback() {
    this.#agentId = this.getAttribute('agent-id');
    if (!this.#agentId) {
      this.innerHTML = '<p class="msg error">No agent ID.</p>';
      return;
    }
    this.innerHTML = '<p class="msg">Loading config…</p>';
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
    this.#render(config, Array.isArray(secrets) ? secrets : []);
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
        return `<option value="${name}"${name === secretName ? ' selected' : ''}>${name}</option>`;
      }),
    ].join('');

    this.innerHTML = `
      <div class="grid-2">
        <div class="row">
          <label>Provider</label>
          <select id="provider"><option value="">— select —</option>${opts(PROVIDERS, provider)}</select>
          <div class="hint">Where the call is routed (outbound).</div>
        </div>
        <div class="row">
          <label>Model</label>
          <input id="model" type="text" value="${model}" placeholder="e.g. gpt-4o-mini" />
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
        <input id="fallback_models" type="text" value="${fallbacks}" placeholder="provider/model, provider/model" />
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
    const body = {
      provider,
      model,
      inbound_format: val('inbound_format'),
      temperature: num('temperature'),
      max_tokens: num('max_tokens'),
      fallback_models: val('fallback_models').split(',').map((s) => s.trim()).filter(Boolean),
      api_key_secret_name: val('api_key_secret_name') || null,
    };

    try {
      await fetchApi(`/agents/${this.#agentId}/llm-config`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      showToast('LLM config saved');
    } catch (e) {
      showToast(`Failed: ${e.message}`);
    }
  }
}

customElements.define('agent-llm-config', AgentLlmConfig);
