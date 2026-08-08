import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';

import styles from './settings-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TABS = [
  { key: 'general', label: 'General', sub: 'Instance identity, defaults, and platform behaviour.' },
  { key: 'models', label: 'Models', sub: 'Provider API keys used for model routing. Stored encrypted.' },
  { key: 'registry', label: 'Registry', sub: 'External OCI registry used for agent images.' },
];

class SettingsPage extends HTMLElement {
  #initialized = false;
  #settings = {};

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <nav class="side-nav">
        <div class="side-nav-title">Settings</div>
        ${TABS.map(t =>
          `<button class="side-nav-item${t.key === 'general' ? ' is-active' : ''}" type="button" data-tab="${t.key}">${t.label}</button>`
        ).join('')}
      </nav>

      <div class="content">
        ${TABS.map(t => `
          <div class="panel-head${t.key === 'general' ? ' is-active' : ''}" data-panel-head="${t.key}">
            <h1 class="title-page">${t.label}</h1>
            <p class="page-sub">${t.sub}</p>
          </div>
        `).join('')}

        <div class="panel is-active" data-panel="general">
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-instance-name">Instance name</label>
              <div class="hint">Shown in the shell and used to identify this control plane.</div>
            </div>
            <div class="setting-control">
              <input type="text" id="s-instance-name" data-field="instance_name" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-default-model">Default model</label>
              <div class="hint">Applied to new agents and sessions unless overridden.</div>
            </div>
            <div class="setting-control">
              <select id="s-default-model" data-field="default_model">
                <option value="claude-sonnet-4-6">Claude Sonnet 4.6</option>
                <option value="claude-haiku-4-5">Claude Haiku 4.5</option>
                <option value="claude-opus-4-6">Claude Opus 4.6</option>
              </select>
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-max-tokens">Max tokens per request</label>
              <div class="hint">Maximum output tokens allowed per API request.</div>
            </div>
            <div class="setting-control">
              <input type="number" id="s-max-tokens" data-field="max_tokens" min="1" max="200000" />
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
        </div>

        <div class="panel" data-panel="models">
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-anthropic-key">Anthropic API key</label>
              <div class="hint">Used for Claude model routing. Stored encrypted.</div>
            </div>
            <div class="setting-control">
              <input type="password" id="s-anthropic-key" data-field="anthropic_api_key" placeholder="sk-ant-..." />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-openai-key">OpenAI API key</label>
              <div class="hint">Optional — enables OpenAI model routing.</div>
            </div>
            <div class="setting-control">
              <input type="password" id="s-openai-key" data-field="openai_api_key" placeholder="sk-..." />
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
              <input type="url" id="s-registry-url" data-field="registry_url" placeholder="https://registry.example.com" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-registry-user">Registry username</label>
            </div>
            <div class="setting-control">
              <input type="text" id="s-registry-user" data-field="registry_username" />
            </div>
          </div>
          <div class="setting-row">
            <div class="setting-info">
              <label for="s-registry-pass">Registry password</label>
            </div>
            <div class="setting-control">
              <input type="password" id="s-registry-pass" data-field="registry_password" />
            </div>
          </div>
        </div>

        <div class="save-bar">
          <button class="save-btn" id="btn-save">Save changes</button>
        </div>
      </div>
    `;

    this.querySelector('.side-nav').addEventListener('click', (e) => {
      const tab = e.target.closest('.side-nav-item');
      if (!tab) return;
      this.querySelectorAll('.side-nav-item').forEach(t => t.classList.toggle('is-active', t === tab));
      this.querySelectorAll('.panel').forEach(p =>
        p.classList.toggle('is-active', p.dataset.panel === tab.dataset.tab));
      this.querySelectorAll('.panel-head').forEach(h =>
        h.classList.toggle('is-active', h.dataset.panelHead === tab.dataset.tab));
    });

    this.querySelector('#btn-save').addEventListener('click', () => this.#save());
    this.#load();
  }

  async #load() {
    const s = await window.fetchSettings();
    if (!s) return;
    this.#settings = s;
    this.querySelectorAll('[data-field]').forEach(el => {
      if (s[el.dataset.field] != null) el.value = s[el.dataset.field];
    });
  }

  #save() {
    const btn = this.querySelector('#btn-save');
    withLoading(btn, 'Saving…', async () => {
      const updated = { ...this.#settings };
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
