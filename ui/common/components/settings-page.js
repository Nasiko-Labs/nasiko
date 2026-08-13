import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';
import '/common/components/app-module-nav.js';

import styles from './settings-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

// Every field below must exist in the server's `SettingsUpdate`
// (oss/server/src/settings.rs). Serde drops unknown keys silently, so a field
// the server doesn't know still returns 200 and still toasts "saved" while
// persisting nothing — this page previously shipped seven such fields
// (instance_name, default_model, max_tokens, anthropic_api_key, openai_api_key,
// registry_username, registry_password) and hid four that the server does
// support. Adding a control here means adding it there too.
const TABS = [
  { key: 'general', label: 'General', sub: 'Routing defaults and platform behaviour.' },
  { key: 'limits', label: 'Flow limits', sub: 'Cascade guards applied to every inter-agent call.' },
  { key: 'registry', label: 'Registry', sub: 'External OCI registry used for agent images.' },
  { key: 'sso', label: 'Single sign-on', sub: 'OIDC provider used for "Continue with Microsoft".' },
];

// Sections are reachable from elsewhere in the module (the nav rows link here
// as /settings.html#<key>), so the hash — not a hardcoded default — picks the
// panel on load.
const sectionFromHash = () => {
  const key = decodeURIComponent(location.hash.slice(1));
  return TABS.some(t => t.key === key) ? key : 'general';
};

class SettingsPage extends HTMLElement {
  #initialized = false;
  #settings = {};

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    const initial = sectionFromHash();

    this.innerHTML = `
      <app-module-nav module="settings" active-section="${initial}"></app-module-nav>

      <div class="content">
        ${TABS.map(t => `
          <div class="panel-head${t.key === initial ? ' is-active' : ''}" data-panel-head="${t.key}">
            <h1 class="title-page">${t.label}</h1>
            <p class="page-sub">${t.sub}</p>
          </div>
        `).join('')}

        <div class="panel is-active" data-panel="general">
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-router-model">Router model</label>
              <div class="hint">Model the routing engine uses to pick an agent for each query (<code>ROUTER_MODEL</code>).</div>
            </div>
            <div class="setting-control">
              <input type="text" id="s-router-model" data-field="router_model" placeholder="e.g. gpt-4o" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-default-provider">Default provider</label>
              <div class="hint">Provider used when an agent has no LLM config of its own.</div>
            </div>
            <div class="setting-control">
              <select id="s-default-provider" data-field="default_provider">
                <option value="openai">OpenAI</option>
                <option value="anthropic">Anthropic</option>
                <option value="gemini">Gemini</option>
              </select>
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-catalog-tabs">Agent catalog tabs</label>
              <div class="hint">Comma-separated agent tags pinned as the catalog's filter tabs. Leave empty to derive tabs from the most common tags across agents.</div>
            </div>
            <div class="setting-control">
              <input type="text" id="s-catalog-tabs" data-field="catalog_tabs" data-allow-empty placeholder="e.g. devops, finance, support" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label>Provider API keys</label>
              <div class="hint">Keys aren't stored here — each routing config references one of your
                encrypted secrets. Manage them on the
                <a href="/llm-router.html">LLM router</a> and <a href="/secrets.html">Secrets</a> pages.</div>
            </div>
            <div class="setting-control"></div>
          </div>
        </div>

        <div class="panel" data-panel="limits">
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-flow-depth">Max call depth</label>
              <div class="hint">How many agent-to-agent hops one flow may chain before it's rejected.</div>
            </div>
            <div class="setting-control">
              <input type="number" id="s-flow-depth" data-field="max_flow_depth" min="1" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-flow-fanout">Max fan-out</label>
              <div class="hint">Maximum agents a single flow may call in total.</div>
            </div>
            <div class="setting-control">
              <input type="number" id="s-flow-fanout" data-field="max_flow_fan_out" min="1" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-flow-tokens">Token budget per flow</label>
              <div class="hint">Combined prompt + completion tokens a flow may spend.</div>
            </div>
            <div class="setting-control">
              <input type="number" id="s-flow-tokens" data-field="max_flow_tokens" min="1" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-flow-timeout">Flow timeout (seconds)</label>
              <div class="hint">Wall-clock limit for a whole flow.</div>
            </div>
            <div class="setting-control">
              <input type="number" id="s-flow-timeout" data-field="flow_timeout_secs" min="1" />
            </div>
          </div>
        </div>

        <div class="panel" data-panel="registry">
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-registry-url">OCI registry URL</label>
              <div class="hint">Where imported agent images are pulled from.</div>
            </div>
            <div class="setting-control">
              <input type="url" id="s-registry-url" data-field="registry_url" data-allow-empty placeholder="https://registry.example.com" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label>Registry credentials</label>
              <div class="hint">Per-agent pull credentials are issued by the platform, and the
                cluster-wide build credential comes from <code>BUILD_PUSH_TOKEN</code> — neither is
                configured from this page.</div>
            </div>
            <div class="setting-control"></div>
          </div>
        </div>

        <div class="panel" data-panel="sso">
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-oidc-issuer">Issuer URL</label>
              <div class="hint">Your IdP's discovery base URL, e.g. <code>https://login.microsoftonline.com/&lt;tenant&gt;/v2.0</code>.</div>
            </div>
            <div class="setting-control">
              <input type="url" id="s-oidc-issuer" data-field="oidc_issuer_url" data-allow-empty />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-oidc-client-id">Client ID</label>
            </div>
            <div class="setting-control">
              <input type="text" id="s-oidc-client-id" data-field="oidc_client_id" data-allow-empty />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-oidc-client-secret">Client secret</label>
              <!-- No data-allow-empty: the server treats an empty string as
                   "clear the secret" and an absent field as "leave it alone",
                   so a blank box must not be submitted. -->
              <div class="hint" id="s-oidc-secret-state">Write-only — leave blank to keep the stored secret.</div>
            </div>
            <div class="setting-control">
              <input type="password" id="s-oidc-client-secret" data-field="oidc_client_secret" placeholder="unchanged" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-oidc-redirect">Redirect URI</label>
              <div class="hint">Must match the IdP registration exactly.</div>
            </div>
            <div class="setting-control">
              <input type="url" id="s-oidc-redirect" data-field="oidc_redirect_uri" data-allow-empty />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-oidc-scopes">Scopes</label>
              <div class="hint">Space-separated. Defaults to <code>openid profile email</code>.</div>
            </div>
            <div class="setting-control">
              <input type="text" id="s-oidc-scopes" data-field="oidc_scopes" data-allow-empty placeholder="openid profile email" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-oidc-label">Button label</label>
              <div class="hint">Overrides the sign-in button text.</div>
            </div>
            <div class="setting-control">
              <input type="text" id="s-oidc-label" data-field="oidc_provider_label" data-allow-empty placeholder="Microsoft" />
            </div>
          </div>
        </div>

        <div class="save-bar">
          <button class="save-btn" id="btn-save">Save changes</button>
        </div>
      </div>
    `;

    this.addEventListener('module-nav-select', (e) => this.#show(e.detail.section));
    // Nav rows are links, so back/forward only changes the hash.
    window.addEventListener('hashchange', () => {
      const key = sectionFromHash();
      this.#show(key);
      this.querySelector('app-module-nav')?.setAttribute('active-section', key);
    });
    this.#show(initial);

    this.querySelector('#btn-save').addEventListener('click', () => this.#save());
    this.#load();
  }

  #show(key) {
    if (!TABS.some(t => t.key === key)) return;
    this.querySelectorAll('.panel').forEach(p =>
      p.classList.toggle('is-active', p.dataset.panel === key));
    this.querySelectorAll('.panel-head').forEach(h =>
      h.classList.toggle('is-active', h.dataset.panelHead === key));
  }

  async #load() {
    const s = await window.fetchSettings();
    if (!s) return;
    this.#settings = s;
    this.querySelectorAll('[data-field]').forEach(el => {
      if (s[el.dataset.field] != null) el.value = s[el.dataset.field];
    });

    // The secret itself is never returned — only whether one is stored.
    const secretState = this.querySelector('#s-oidc-secret-state');
    if (secretState) {
      secretState.textContent = s.oidc_client_secret_configured
        ? 'A secret is stored. Leave blank to keep it, or enter a new one to replace it.'
        : 'No secret stored yet — SSO stays disabled until one is set.';
    }
  }

  #save() {
    const btn = this.querySelector('#btn-save');
    withLoading(btn, 'Saving…', async () => {
      const updated = { ...this.#settings };
      // Read-only on the wire: sending it back is harmless (serde ignores it)
      // but dropping it keeps the payload honest about what it's asking to set.
      delete updated.oidc_client_secret_configured;
      this.querySelectorAll('[data-field]').forEach(el => {
        const v = el.value.trim();
        // data-allow-empty fields round-trip '' so they can be cleared.
        if (v || el.hasAttribute('data-allow-empty')) {
          updated[el.dataset.field] = el.type === 'number' ? Number(v) : v;
        }
      });
      await window.saveSettings(updated);
      showToast('Settings saved');
    })();
  }
}

customElements.define('settings-page', SettingsPage);
