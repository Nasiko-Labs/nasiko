import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';
import { initialView, syncView } from '/common/utils/module-view.js';

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

// The key web/settings.html gives this element as a `data-view` of the Settings
// module-shell. The shell selects between *views* (this page and Secrets); the
// four sections above are a finer level inside this one view, which is why they
// are not view keys and why this element is the shell's `default-view` — a
// `?view=limits` link means "Settings, Flow limits section", and only the
// fallback-to-default keeps the shell showing this page for it.
const VIEW = 'settings';

class SettingsPage extends HTMLElement {
  #initialized = false;
  #settings = {};
  /** The section on screen — one of TABS' keys. */
  #section = TABS[0].key;
  /** Where the nav's bubbling events are listened for (see connectedCallback). */
  #navRoot = null;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    // Deep links name a section, not a view: `?view=limits` opens this page on
    // Flow limits. Same param and same validator as the shell's, so the two
    // levels cannot disagree about how a view is spelled — an unknown value
    // falls back to General rather than rendering four hidden panels.
    this.#section = initialView(TABS.map(t => t.key), TABS[0].key);

    this.innerHTML = `
      <div class="content">
        ${TABS.map(t => `
          <div class="panel-head${t.key === this.#section ? ' is-active' : ''}" data-panel-head="${t.key}">
            <h1 class="title-page">${t.label}</h1>
            <p class="page-sub">${t.sub}</p>
          </div>
        `).join('')}

        <div class="panel${this.#section === 'general' ? ' is-active' : ''}" data-panel="general">
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
                <a href="/llm-router.html">LLM router</a> and
                <a href="/settings.html?view=secrets">Secrets</a> pages.</div>
            </div>
            <div class="setting-control"></div>
          </div>
        </div>

        <div class="panel${this.#section === 'limits' ? ' is-active' : ''}" data-panel="limits">
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

        <div class="panel${this.#section === 'registry' ? ' is-active' : ''}" data-panel="registry">
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

        <div class="panel${this.#section === 'sso' ? ' is-active' : ''}" data-panel="sso">
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

    // The nav is the shell's child, not this element's (see web/settings.html),
    // so its bubbling `module-nav-select` goes past this element rather than
    // through it — listen on the shell. Standalone hosts (an element created in
    // JS, or a page that still renders its own nav inside) keep the old target.
    this.#navRoot = this.closest('module-shell') ?? this;
    this.#navRoot.addEventListener('module-nav-select', this.#onNavSelect);

    // Only meaningful for the shell's own view keys, and the shell resolves
    // those itself; here it is the section level that has to reach the nav.
    if (this.#isActiveView()) this.#highlightNav(this.#section);

    this.querySelector('#btn-save').addEventListener('click', () => this.#save());
    this.#load();
  }

  disconnectedCallback() {
    this.#navRoot?.removeEventListener('module-nav-select', this.#onNavSelect);
  }

  /** True when the shell is showing this view (or there is no shell). */
  #isActiveView() {
    const shell = this.closest('module-shell');
    return !shell || shell.activeView === VIEW;
  }

  /** The nav lives beside this element now, and only this element knows which
   *  section is up, so the highlight at that granularity is ours to set. */
  #highlightNav(key) {
    (this.closest('module-shell') ?? this)
      .querySelector('app-module-nav')
      ?.setAttribute('active-section', key);
  }

  /**
   * One event, two granularities. The shell answers it for its view keys and
   * ignores everything else, and these four section keys are exactly that
   * "everything else" — so a workspace row never moves the shell, and this
   * element owns the switch.
   */
  #onNavSelect = (e) => {
    const key = e.detail?.section;
    if (!TABS.some(t => t.key === key)) return;
    this.#section = key;
    this.querySelectorAll('.panel').forEach(p =>
      p.classList.toggle('is-active', p.dataset.panel === key));
    this.querySelectorAll('.panel-head').forEach(h =>
      h.classList.toggle('is-active', h.dataset.panelHead === key));

    // Arriving from a sibling view (Secrets): the shell is still showing that
    // one, since the key it just saw is not one of its own, so ask it for this
    // view. `show()` then names *its* coarser key in the URL and on the nav, so
    // put the section the user actually clicked back into both.
    const shell = this.closest('module-shell');
    if (shell && shell.activeView !== VIEW) {
      shell.show(VIEW);
      syncView(key);
      this.#highlightNav(key);
    }
  };

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
