import { icons } from '../utils/icons.js';
import { fetchApi } from '../services/api.js';
import { showToast } from '../utils/toast.js';
import { confirmDialog } from '../utils/confirm-dialog.js';
import { attachSlidingIndicator } from '../utils/tab-indicator.js';
import styles from './mcp-detail-page.css' with { type: 'css' };
import './app-skeleton.js';
import './autocomplete.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const AUTH_LABELS = {
  none: 'No auth',
  bearer: 'API key',
  basic: 'Basic',
  oauth2: 'OAuth 2.1',
  url_param: 'URL param',
};

const TABS = [
  { key: 'overview', label: 'Overview' },
  { key: 'access', label: 'Access & security' },
  { key: 'logs', label: 'Logs', buildOnly: true },
  { key: 'settings', label: 'Settings', ownerOnly: true },
];

class McpDetailPage extends HTMLElement {
  #initialized = false;
  #connectorId = null;
  #connector = null;
  #tools = [];
  #agents = [];
  #selectedAgentId = '';
  #agentConnectors = [];
  #agentTools = new Map();

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#connectorId = new URLSearchParams(location.search).get('id');
    if (!this.#connectorId) {
      this.innerHTML = '<p style="padding:var(--space-xl);color:var(--color-text-muted);">No connector ID specified</p>';
      return;
    }
    this.innerHTML = '<app-skeleton height="400px" style="max-width:900px;margin:0 auto;"></app-skeleton>';
    this.#load();
  }

  async #load() {
    try {
      const resp = await window.fetchMcpConnectorDetail(this.#connectorId);
      this.#connector = resp?.data ?? resp;
    } catch {
      this.#connector = null;
    }
    if (!this.#connector?.name && !this.#connector?.display_name) {
      this.innerHTML = '<p style="color:var(--color-error);padding:var(--space-xl);">Connector not found</p>';
      return;
    }
    document.title = 'Nasiko - ' + (this.#connector.display_name || this.#connector.name);
    await this.#loadTools();
    this.#loadAgents();
    this.#render();
  }

  async #loadTools() {
    try {
      if (Array.isArray(this.#connector.tools)) {
        this.#tools = this.#connector.tools;
        return;
      }
      const resp = await fetchApi('/mcp/connectors/' + encodeURIComponent(this.#connectorId) + '/tools');
      this.#tools = resp?.data?.tools || resp?.data || [];
    } catch {
      this.#tools = [];
    }
  }

  async #loadAgents() {
    try {
      const resp = await window.fetchAgents('', 1, 100);
      this.#agents = resp?.data || [];
    } catch {
      this.#agents = [];
    }
  }

  #connectorStatus(c) {
    if (c.source_kind === 'uploaded_build') {
      if (c.build_status === 'pending' || c.build_status === 'building') {
        return { cls: 'mdp-badge--building', label: 'Building' };
      }
      if (c.build_status === 'failed') return { cls: 'mdp-badge--failed', label: 'Failed' };
    }
    return c.is_active
      ? { cls: 'mdp-badge--active', label: 'Active' }
      : { cls: 'mdp-badge--inactive', label: 'Inactive' };
  }

  // ── Render ────────────────────────────────────────────────────────────────

  #render() {
    const c = this.#connector;
    const name = c.display_name || c.name;
    const st = this.#connectorStatus(c);
    const isOwner = !!c.is_owner;
    const isBuild = c.source_kind === 'uploaded_build';
    const tabs = TABS.filter((t) => (!t.ownerOnly || isOwner) && (!t.buildOnly || isBuild));

    this.innerHTML = `
      <div class="mdp-page">
        <div class="mdp-topbar">
          <a class="mdp-back" href="/mcp.html">${icons.x('', 16)}</a>
        </div>

        <div class="mdp-header">
          <div class="mdp-title-row">
            <h1 class="mdp-name">${this.#esc(name)}</h1>
            <span class="mdp-badge ${st.cls}"><span class="mdp-badge-dot"></span>${this.#esc(st.label)}</span>
          </div>
          ${c.description ? '<p class="mdp-description">' + this.#esc(c.description) + '</p>' : ''}
        </div>

        <nav class="mdp-tabs">
          ${tabs.map((t, i) => '<button class="mdp-tab' + (i === 0 ? ' is-active' : '') + '" data-tab="' + t.key + '">' + t.label + '</button>').join('')}
        </nav>

        ${this.#overviewPanelHtml(c)}
        ${this.#accessPanelHtml()}
        ${isBuild ? this.#logsPanelHtml() : ''}
        ${isOwner ? this.#settingsPanelHtml() : ''}
      </div>
    `;

    this.#wireTabs();
    this.#wireAgentPicker();
    if (isOwner) this.#wireDeleteBtn();
    if (c.auth_type === 'oauth2') this.#renderOauthSection(c);
    else if (c.auth_type && c.auth_type !== 'none') this.#renderCredentialSection(c);
  }

  #wireTabs() {
    let logsLoaded = false;
    attachSlidingIndicator(this.querySelector('.mdp-tabs'), '.mdp-tab', '.is-active');
    this.querySelector('.mdp-tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.mdp-tab');
      if (!tab) return;
      this.querySelectorAll('.mdp-tab').forEach((t) => t.classList.remove('is-active'));
      this.querySelectorAll('.mdp-panel').forEach((p) => p.classList.remove('is-active'));
      tab.classList.add('is-active');
      this.querySelector('[data-panel="' + tab.dataset.tab + '"]')?.classList.add('is-active');
      if (tab.dataset.tab === 'logs' && !logsLoaded) {
        logsLoaded = true;
        this.#loadLogs();
      }
    });
  }

  async #loadLogs() {
    const pre = this.querySelector('#mdp-logs-pre');
    if (!pre) return;
    try {
      const resp = await window.fetchMcpBuildLogs(this.#connectorId, 500);
      const logs = typeof resp?.data === 'string' ? resp.data : (resp?.data ?? '');
      pre.textContent = logs || '(no logs)';
    } catch (e) {
      pre.textContent = 'Failed to load logs: ' + e.message;
    }
  }

  // ── Overview panel ────────────────────────────────────────────────────────

  #overviewPanelHtml(c) {
    const items = [
      ['URL', c.url || '--'],
      ['Transport', c.transport || 'stdio'],
      ['Auth type', AUTH_LABELS[c.auth_type] || c.auth_type || '--'],
      ['Version', c.version || '--'],
      ['Source', c.source_kind === 'uploaded_build' ? 'Uploaded build' : 'Registered'],
      ['Owner', c.owner_username || '--'],
      ['Tools', String(c.tool_count ?? this.#tools.length)],
      ['Created', c.created_at ? new Date(c.created_at).toLocaleString() : '--'],
    ];
    return `
      <div class="mdp-panel is-active" data-panel="overview">
        <div class="mdp-meta-grid">
          ${items.map(([label, value]) => '<div class="mdp-meta-item"><span class="mdp-meta-label">' + this.#esc(label) + '</span><span class="mdp-meta-value">' + this.#esc(value) + '</span></div>').join('')}
        </div>

        ${this.#toolsSectionHtml()}

        <div id="mdp-auth-section"></div>
      </div>`;
  }

  #toolsSectionHtml() {
    if (!this.#tools.length) {
      return '<div class="mdp-section"><h2 class="mdp-section-title">Tools (0)</h2><p class="mdp-muted">No tools discovered yet</p></div>';
    }
    return `
      <div class="mdp-section">
        <h2 class="mdp-section-title">Tools (${this.#tools.length})</h2>
        <div class="mdp-tools-grid">
          ${this.#tools.map((t) => '<div class="mdp-tool-card"><span class="mdp-tool-name">' + this.#esc(t.name) + '</span>' + (t.description ? '<span class="mdp-tool-desc">' + this.#esc(t.description) + '</span>' : '') + '</div>').join('')}
        </div>
      </div>`;
  }

  #logsPanelHtml() {
    return `
      <div class="mdp-panel" data-panel="logs">
        <div class="mdp-section">
          <pre class="mdp-logs-pre" id="mdp-logs-pre">Loading logs...</pre>
        </div>
      </div>`;
  }

  // ── Credential / OAuth ────────────────────────────────────────────────────

  async #renderCredentialSection(c) {
    const section = this.querySelector('#mdp-auth-section');
    if (!section) return;
    let connected = false;
    try {
      const resp = await window.fetchMcpCredentialStatus(c.connector_id);
      connected = !!resp?.data?.connected;
    } catch { /* leave disconnected */ }
    section.innerHTML = `
      <div class="mdp-section">
        <h2 class="mdp-section-title">${icons.key('', 16)} Credential</h2>
        <div class="mdp-cred-row">
          <span class="mdp-status-dot ${connected ? 'is-ok' : 'is-off'}"></span>
          <span>${connected ? 'Credential set' : 'No credential set'}</span>
          ${connected ? '<button class="mdp-btn-ghost danger" id="mdp-cred-remove">Remove</button>' : ''}
        </div>
        <div class="mdp-cred-form">
          <input type="password" id="mdp-cred-value" class="mdp-input" placeholder="${c.auth_type === 'basic' ? 'username:password' : 'API key / token'}" />
          <button class="mdp-btn-dark" id="mdp-cred-save">${connected ? 'Replace' : 'Save'}</button>
        </div>
        <p class="mdp-form-error" id="mdp-cred-error" hidden></p>
      </div>`;
    section.querySelector('#mdp-cred-save').addEventListener('click', async () => {
      const value = section.querySelector('#mdp-cred-value').value.trim();
      if (!value) return;
      const err = section.querySelector('#mdp-cred-error');
      err.hidden = true;
      try {
        const resp = await window.setMcpCredential(c.connector_id, value);
        if (resp?.data?.connected === false) {
          err.textContent = 'Stored, but verification failed: ' + (resp.data.error || 'unknown error');
          err.hidden = false;
        }
        this.#renderCredentialSection(c);
      } catch (e) {
        err.textContent = 'Failed to save: ' + e.message;
        err.hidden = false;
      }
    });
    section.querySelector('#mdp-cred-remove')?.addEventListener('click', async () => {
      try {
        await window.deleteMcpCredential(c.connector_id);
        this.#renderCredentialSection(c);
      } catch (e) { showToast('Remove failed: ' + e.message); }
    });
  }

  async #renderOauthSection(c) {
    const section = this.querySelector('#mdp-auth-section');
    if (!section) return;
    let status = { authorized: false, expires_at: null };
    try {
      const resp = await window.fetchMcpOauthStatus(c.connector_id);
      status = resp?.data ?? status;
    } catch { /* leave unauthorized */ }
    const expiry = status.expires_at ? ' - expires ' + new Date(status.expires_at).toLocaleString() : '';
    section.innerHTML = `
      <div class="mdp-section">
        <h2 class="mdp-section-title">${icons.shield('', 16)} OAuth 2.1</h2>
        <div class="mdp-cred-row">
          <span class="mdp-status-dot ${status.authorized ? 'is-ok' : 'is-off'}"></span>
          <span>${status.authorized ? 'Authorized' + this.#esc(expiry) : 'Not authorized'}</span>
          ${status.authorized
            ? '<button class="mdp-btn-ghost danger" id="mdp-oauth-revoke">Revoke</button>'
            : '<button class="mdp-btn-dark" id="mdp-oauth-authorize">' + icons.externalLink('', 14) + ' Authorize</button>'}
        </div>
        <p class="mdp-form-error" id="mdp-oauth-error" hidden></p>
      </div>`;
    section.querySelector('#mdp-oauth-authorize')?.addEventListener('click', async () => {
      const err = section.querySelector('#mdp-oauth-error');
      err.hidden = true;
      try {
        const resp = await window.authorizeMcpOauth(c.connector_id);
        const url = resp?.data?.authorization_url;
        if (url) window.open(url, 'mcp-oauth', 'width=600,height=720');
      } catch (e) {
        err.textContent = 'Authorization failed: ' + e.message;
        err.hidden = false;
      }
    });
    section.querySelector('#mdp-oauth-revoke')?.addEventListener('click', async () => {
      try {
        await window.revokeMcpOauthToken(c.connector_id);
        this.#renderOauthSection(c);
      } catch (e) { showToast('Revoke failed: ' + e.message); }
    });
  }

  // ── Access & security panel ───────────────────────────────────────────────

  #accessPanelHtml() {
    return `
      <div class="mdp-panel" data-panel="access">
        <div class="mdp-section">
          <h2 class="mdp-section-title">Agent access</h2>
          <p class="mdp-muted">Select an agent to manage its access to this connector and set per-tool allow/block rules</p>
          <div class="mdp-agent-picker">
            <auto-complete id="mdp-agent-select" placeholder="Search agents..." aria-label="Agent"></auto-complete>
          </div>
          <div id="mdp-agent-access-body">
            <div class="mdp-empty">${icons.network('', 28)}<p>Select an agent above</p></div>
          </div>
        </div>
      </div>`;
  }

  #wireAgentPicker() {
    const picker = this.querySelector('#mdp-agent-select');
    if (!picker) return;
    picker.filterFn = (query) => {
      const q = query.toLowerCase();
      return this.#agents
        .filter((a) => !q || (a.display_name || '').toLowerCase().includes(q) || (a.name || '').toLowerCase().includes(q))
        .map((a) => ({
          label: a.display_name || a.name,
          subtitle: a.name !== (a.display_name || a.name) ? a.name : (a.status || ''),
          value: a.id,
        }));
    };
    picker.addEventListener('option-selected', (e) => {
      this.#selectedAgentId = e.detail.value;
      this.#loadAgentAccess();
    });
    picker.addEventListener('input', () => {
      if (picker.value.trim() === '' && this.#selectedAgentId) {
        this.#selectedAgentId = '';
        const body = this.querySelector('#mdp-agent-access-body');
        if (body) body.innerHTML = '<div class="mdp-empty">' + icons.network('', 28) + '<p>Select an agent above</p></div>';
      }
    });
  }

  async #loadAgentAccess() {
    const body = this.querySelector('#mdp-agent-access-body');
    if (!body || !this.#selectedAgentId) return;
    body.innerHTML = '<app-skeleton lines="3" style="padding:var(--space-md)"></app-skeleton>';
    try {
      const resp = await window.fetchAgentMcpConnectors(this.#selectedAgentId);
      this.#agentConnectors = resp?.data?.connectors || [];
    } catch (e) {
      body.innerHTML = '<div class="mdp-empty"><p>Failed to load: ' + this.#esc(e.message) + '</p></div>';
      return;
    }
    this.#agentTools = new Map();
    const match = this.#agentConnectors.find((c) => c.connector_id === this.#connectorId);
    this.#renderAgentAccess(match);
  }

  #renderAgentAccess(match) {
    const body = this.querySelector('#mdp-agent-access-body');
    if (!body) return;
    const enabled = match ? !!match.enabled : false;
    body.innerHTML = `
      <div class="mdp-access-row">
        <label class="mdp-switch">
          <input type="checkbox" class="mdp-access-toggle" ${enabled ? 'checked' : ''} />
          <span class="mdp-slider"></span>
        </label>
        <span>${enabled ? 'Enabled' : 'Disabled'}</span>
      </div>
      ${enabled ? '<button class="mdp-btn-ghost" id="mdp-tools-toggle">' + icons.chevronDown('', 14) + ' Tool rules</button><div id="mdp-tools-editor" hidden></div>' : ''}`;

    body.querySelector('.mdp-access-toggle')?.addEventListener('change', async (e) => {
      try {
        await window.setAgentMcpConnectorAccess(this.#selectedAgentId, this.#connectorId, e.target.checked);
        this.#loadAgentAccess();
      } catch (err) {
        e.target.checked = !e.target.checked;
        showToast('Failed to update access: ' + err.message);
      }
    });

    body.querySelector('#mdp-tools-toggle')?.addEventListener('click', () => {
      const editor = body.querySelector('#mdp-tools-editor');
      if (!editor) return;
      editor.hidden = !editor.hidden;
      if (!editor.hidden) this.#renderToolsEditor();
    });
  }

  async #renderToolsEditor() {
    const editor = this.querySelector('#mdp-tools-editor');
    if (!editor) return;
    if (!this.#agentTools.has(this.#connectorId)) {
      editor.innerHTML = '<app-skeleton lines="3"></app-skeleton>';
      try {
        const resp = await window.fetchAgentMcpConnectorTools(this.#selectedAgentId, this.#connectorId);
        this.#agentTools.set(this.#connectorId, resp?.data?.tools || []);
      } catch (e) {
        editor.innerHTML = '<p class="mdp-form-error">Failed to load tools: ' + this.#esc(e.message) + '</p>';
        return;
      }
    }
    const tools = this.#agentTools.get(this.#connectorId);
    if (!tools.length) {
      editor.innerHTML = '<p class="mdp-muted">No tools to configure</p>';
      return;
    }
    editor.innerHTML = `
      <div class="mdp-tools-list">
        ${tools.map((t, i) => '<div class="mdp-tool-line"><div class="mdp-tool-info"><span class="mdp-tool-info-name">' + this.#esc(t.name) + '</span>' + (t.description ? '<span class="mdp-tool-info-desc">' + this.#esc(t.description) + '</span>' : '') + '</div><select class="mdp-tool-stance" data-index="' + i + '"><option value="allow"' + (t.stance !== 'block' ? ' selected' : '') + '>Allow</option><option value="block"' + (t.stance === 'block' ? ' selected' : '') + '>Block</option></select></div>').join('')}
      </div>
      <div class="mdp-tools-actions">
        <span class="mdp-save-status" id="mdp-save-status" hidden></span>
        <button class="mdp-btn-dark" id="mdp-save-rules">Save rules</button>
      </div>`;
    editor.querySelector('#mdp-save-rules').addEventListener('click', async () => {
      const rules = [...editor.querySelectorAll('.mdp-tool-stance')].map((sel) => ({
        connector_id: this.#connectorId,
        tool_pattern: tools[Number(sel.dataset.index)].name,
        stance: sel.value,
      }));
      const statusEl = editor.querySelector('#mdp-save-status');
      try {
        await window.saveAgentMcpToolRules(this.#selectedAgentId, rules);
        statusEl.textContent = 'Saved';
        statusEl.className = 'mdp-save-status is-ok';
      } catch (e) {
        statusEl.textContent = 'Save failed: ' + e.message;
        statusEl.className = 'mdp-save-status is-error';
      }
      statusEl.hidden = false;
    });
  }

  // ── Settings panel ────────────────────────────────────────────────────────

  #settingsPanelHtml() {
    return `
      <div class="mdp-panel" data-panel="settings">
        <div class="mdp-section mdp-danger">
          <h3 class="mdp-danger-title">Danger zone</h3>
          <p class="mdp-muted">Deleting this connector removes it and revokes all agent access</p>
          <button class="mdp-btn-danger" id="mdp-delete-btn">${icons.trash('', 14)} Delete connector</button>
        </div>
      </div>`;
  }

  #wireDeleteBtn() {
    this.querySelector('#mdp-delete-btn')?.addEventListener('click', async () => {
      const name = this.#connector.display_name || this.#connector.name;
      const confirmed = await confirmDialog({
        title: 'Delete ' + name,
        message: 'This removes the connector and revokes all agent access. This cannot be undone.',
        confirmLabel: 'Delete',
        danger: true,
      });
      if (!confirmed) return;
      try {
        await window.deleteMcpConnector(this.#connectorId);
        location.href = '/mcp.html';
      } catch (e) {
        showToast('Failed to delete: ' + e.message);
      }
    });
  }

  // ── Utilities ─────────────────────────────────────────────────────────────

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('mcp-detail-page', McpDetailPage);