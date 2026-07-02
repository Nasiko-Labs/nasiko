import { showToast } from '/common/utils/toast.js';
import { withLoading } from '/common/utils/async-button.js';

import styles from './settings-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TABS = [
  { key: 'general', label: 'General' },
  { key: 'models', label: 'Models' },
  { key: 'registry', label: 'Registry' },
];

class SettingsPage extends HTMLElement {
  #settings = {};

  connectedCallback() {
    this.innerHTML = `
      <h1 class="page-title">Settings</h1>
      <nav class="tabs">${TABS.map(t =>
        `<button class="tab${t.key === 'general' ? ' is-active' : ''}" data-tab="${t.key}">${t.label}</button>`
      ).join('')}</nav>

      <div class="panel is-active" data-panel="general">
        <div class="form-group">
          <label for="s-instance-name">Instance Name</label>
          <input type="text" id="s-instance-name" data-field="instance_name" />
        </div>
        <div class="form-group">
          <label for="s-default-model">Default Model</label>
          <select id="s-default-model" data-field="default_model">
            <option value="claude-sonnet-4-6">Claude Sonnet 4.6</option>
            <option value="claude-haiku-4-5">Claude Haiku 4.5</option>
            <option value="claude-opus-4-6">Claude Opus 4.6</option>
          </select>
        </div>
        <div class="form-group">
          <label for="s-max-tokens">Max Tokens per Request</label>
          <input type="number" id="s-max-tokens" data-field="max_tokens" min="1" max="200000" />
          <div class="hint">Maximum output tokens allowed per API request.</div>
        </div>
      </div>

      <div class="panel" data-panel="models">
        <div class="form-group">
          <label for="s-anthropic-key">Anthropic API Key</label>
          <input type="password" id="s-anthropic-key" data-field="anthropic_api_key" placeholder="sk-ant-..." />
          <div class="hint">Used for Claude model routing. Stored encrypted.</div>
        </div>
        <div class="form-group">
          <label for="s-openai-key">OpenAI API Key (optional)</label>
          <input type="password" id="s-openai-key" data-field="openai_api_key" placeholder="sk-..." />
        </div>
      </div>

      <div class="panel" data-panel="registry">
        <div class="form-group">
          <label for="s-registry-url">OCI Registry URL</label>
          <input type="url" id="s-registry-url" data-field="registry_url" placeholder="https://registry.example.com" />
        </div>
        <div class="form-group">
          <label for="s-registry-user">Registry Username</label>
          <input type="text" id="s-registry-user" data-field="registry_username" />
        </div>
        <div class="form-group">
          <label for="s-registry-pass">Registry Password</label>
          <input type="password" id="s-registry-pass" data-field="registry_password" />
        </div>
      </div>

      <div class="save-bar">
        <button class="save-btn" id="btn-save">Save Changes</button>
      </div>
    `;

    this.querySelector('.tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.tab');
      if (!tab) return;
      this.querySelectorAll('.tab').forEach(t => t.classList.remove('is-active'));
      this.querySelectorAll('.panel').forEach(p => p.classList.remove('is-active'));
      tab.classList.add('is-active');
      this.querySelector(`[data-panel="${tab.dataset.tab}"]`).classList.add('is-active');
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
        if (v) updated[el.dataset.field] = el.type === 'number' ? Number(v) : v;
      });
      await window.saveSettings(updated);
      showToast('Settings saved');
    })();
  }
}

customElements.define('settings-page', SettingsPage);
