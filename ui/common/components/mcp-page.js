/**
 * MCP gateway — external MCP connectors, upload-your-own servers, and
 * per-agent tool access.
 *
 * @element mcp-page
 * @note Data sources (all under /api/mcp, envelope {data, status_code, message}):
 *       `window.fetchMcpConnectors()` → GET /api/mcp/connectors
 *         → data = {created_by_you: [dto], shared_with_you: [dto], total}
 *       `window.fetchMcpMyUploads()` → GET /api/mcp/connectors/my-uploads
 *       plus register/probe/update/delete, credential + OAuth management,
 *       upload (zip/GitHub) + build status/logs, and the per-agent
 *       connector/tool-rule endpoints — see oss/ui/web/navigation.js.
 */
import styles from './mcp-page.css' with { type: 'css' };
import { icons } from '../utils/icons.js';
import './app-modal.js';
import './app-skeleton.js';
import './app-module-nav.js';
import './autocomplete.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const AUTH_LABELS = {
  none: 'No auth',
  bearer: 'API key',
  basic: 'Basic',
  oauth2: 'OAuth 2.1',
  url_param: 'URL param',
};

class McpPage extends HTMLElement {
  #initialized = false;
  #connectors = [];
  #uploads = [];
  #agents = [];
  #selectedAgentId = '';
  #agentConnectors = [];
  #agentTools = new Map(); // connector_id → [{name, description, stance}]

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <app-module-nav module="mcp"></app-module-nav>
      <div class="page-head">
        <div>
          <h1 class="page-title">MCP gateway</h1>
          <p class="page-sub">Connect external MCP servers and control which agents can use their tools</p>
        </div>
        <div class="head-actions">
          <button class="btn-outline" id="upload-btn" type="button">${icons.upload('', 14)} Upload MCP server</button>
          <button class="btn-dark" id="register-btn" type="button">${icons.plus('', 14)} Register connector</button>
        </div>
      </div>

      <h2 class="section-title">Connectors</h2>
      <div id="connectors-area">
        <div class="table-wrap">
          <table>
            <thead><tr>
              <th>Connector</th><th>URL</th><th>Transport</th><th>Auth</th>
              <th>Tools</th><th>Status</th><th class="th-actions"></th>
            </tr></thead>
            <tbody id="connectors-tbody">
              <tr class="skel-row" aria-hidden="true"><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td></tr><tr class="skel-row" aria-hidden="true"><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td></tr><tr class="skel-row" aria-hidden="true"><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td><td><app-skeleton height="0.9rem"></app-skeleton></td></tr>
            </tbody>
          </table>
        </div>
      </div>

      <div id="uploads-section" hidden>
        <h2 class="section-title">My uploads</h2>
        <div class="table-wrap">
          <table>
            <thead><tr>
              <th>Name</th><th>Status</th><th>Message</th><th>Endpoint</th><th class="th-actions"></th>
            </tr></thead>
            <tbody id="uploads-tbody"></tbody>
          </table>
        </div>
        <div class="logs-panel" id="logs-panel" hidden>
          <div class="logs-head">
            <span class="logs-title">${icons.terminal('', 14)} Build logs — <span id="logs-name"></span></span>
            <button class="btn-ghost" id="logs-close" type="button">${icons.x('', 14)}</button>
          </div>
          <pre id="logs-pre"></pre>
        </div>
      </div>

      <h2 class="section-title">Agent access</h2>
      <div class="agent-access-card">
        <div class="agent-picker">
          <label for="agent-select">Agent</label>
          <auto-complete id="agent-select" placeholder="Search agents…" aria-label="Agent"></auto-complete>
          <span class="agent-picker-hint">Choose which connectors this agent may call, and set per-tool allow/deny rules.</span>
        </div>
        <div id="agent-access-body">
          <div class="agent-access-empty">${icons.network('', 28)}<p>Select an agent to manage its connector access</p></div>
        </div>
      </div>

      ${this.#registerModalHtml()}
      ${this.#uploadModalHtml()}
      <app-modal heading="Connector" id="detail-modal" hide-footer>
        <div id="detail-body"></div>
      </app-modal>
    `;

    // Module tree nav sections scroll to their page block ("My uploads" only
    // renders once uploads exist — its row no-ops until then).
    this.addEventListener('module-nav-select', (e) => {
      const target = {
        connectors: '#connectors-area',
        uploads: '#uploads-section',
        'agent-access': '.agent-access-card',
      }[e.detail.section];
      const el = target && this.querySelector(target);
      if (el && !el.hidden) el.scrollIntoView({ behavior: 'smooth', block: 'start' });
    });

    this.querySelector('#register-btn').addEventListener('click', () => this.#openRegister());
    this.querySelector('#upload-btn').addEventListener('click', () => this.#openUpload());
    this.querySelector('#logs-close').addEventListener('click', () => {
      this.querySelector('#logs-panel').hidden = true;
    });
    const agentPicker = this.querySelector('#agent-select');
    agentPicker.filterFn = (query) => {
      const q = query.toLowerCase();
      return this.#agents
        .filter((a) => !q
          || (a.display_name || '').toLowerCase().includes(q)
          || (a.name || '').toLowerCase().includes(q))
        .map((a) => ({
          label: a.display_name || a.name,
          subtitle: a.name !== (a.display_name || a.name) ? a.name : (a.status || ''),
          value: a.id,
        }));
    };
    agentPicker.addEventListener('option-selected', (e) => {
      this.#selectedAgentId = e.detail.value;
      this.#loadAgentAccess();
    });
    // Clearing the input deselects the agent and returns to the empty state.
    agentPicker.addEventListener('input', () => {
      if (agentPicker.value.trim() === '' && this.#selectedAgentId) {
        this.#selectedAgentId = '';
        this.#loadAgentAccess();
      }
    });
    this.#wireRegisterModal();
    this.#wireUploadModal();

    this.#load();
    this.#loadAgents();
  }

  // ── Data loading ─────────────────────────────────────────────────────────

  async #load() {
    try {
      const [connResp, uploadsResp] = await Promise.all([
        window.fetchMcpConnectors(),
        window.fetchMcpMyUploads().catch(() => ({ data: [] })),
      ]);
      const d = connResp?.data ?? {};
      this.#connectors = [
        ...(d.created_by_you || []),
        ...(d.shared_with_you || []).map((c) => ({ ...c, __shared: true })),
      ];
      this.#uploads = uploadsResp?.data || [];
    } catch (e) {
      console.error('MCP connectors fetch failed:', e);
      this.querySelector('#connectors-tbody').innerHTML =
        `<tr class="empty-row"><td colspan="7">Failed to load connectors</td></tr>`;
      return;
    }
    this.#renderConnectors();
    this.#renderUploads();
  }

  async #loadAgents() {
    try {
      const resp = await window.fetchAgents('', 1, 100);
      this.#agents = resp?.data || [];
    } catch {
      this.#agents = [];
    }
  }

  // ── Connectors table ─────────────────────────────────────────────────────

  #connectorStatus(c) {
    if (c.source_kind === 'uploaded_build') {
      if (c.build_status === 'pending' || c.build_status === 'building') {
        return { cls: 'is-building', label: 'Building' };
      }
      if (c.build_status === 'failed') return { cls: 'is-error', label: 'Failed' };
    }
    return c.is_active
      ? { cls: 'is-ok', label: 'Active' }
      : { cls: 'is-off', label: 'Inactive' };
  }

  #renderConnectors() {
    const area = this.querySelector('#connectors-area');
    if (!this.#connectors.length) {
      area.innerHTML = `
        <div class="empty-state">
          ${icons.network('', 32)}
          <h3>No connectors yet</h3>
          <p>Register an external MCP server or upload your own to give agents new tools.</p>
          <button class="btn-dark" id="empty-register-btn" type="button">${icons.plus('', 14)} Register connector</button>
        </div>`;
      area.querySelector('#empty-register-btn')
        .addEventListener('click', () => this.#openRegister());
      return;
    }
    // Rebuild the whole table each time — the empty state may have replaced it.
    area.innerHTML = `
      <div class="table-wrap">
        <table>
          <thead><tr>
            <th>Connector</th><th>URL</th><th>Transport</th><th>Auth</th>
            <th>Tools</th><th>Status</th><th class="th-actions"></th>
          </tr></thead>
          <tbody id="connectors-tbody"></tbody>
        </table>
      </div>`;
    const tbody = this.querySelector('#connectors-tbody');
    tbody.innerHTML = this.#connectors.map((c) => {
      const st = this.#connectorStatus(c);
      const name = c.display_name || c.name;
      return `
        <tr data-id="${this.#esc(c.connector_id)}" class="connector-row">
          <td>
            <div class="cell-name">
              <span class="name-main">${this.#esc(name)}</span>
              <span class="name-sub">${this.#esc(c.name)}${c.__shared ? ` · shared by ${this.#esc(c.owner_username || 'someone')}` : ''}</span>
            </div>
          </td>
          <td class="cell-url" title="${this.#esc(c.url || '')}">${this.#esc(c.url || '—')}</td>
          <td>${this.#esc(c.transport || '—')}</td>
          <td><span class="badge badge-auth">${this.#esc(AUTH_LABELS[c.auth_type] || c.auth_type || '—')}</span></td>
          <td>${c.tool_count ?? 0}</td>
          <td><span class="status"><span class="status-dot ${st.cls}"></span>${st.label}</span></td>
          <td class="cell-actions">
            <button class="btn-ghost act-manage" type="button" title="Manage">${icons.settings('', 14)}</button>
            ${c.is_owner ? `<button class="btn-ghost act-delete" type="button" title="Delete">${icons.trash('', 14)}</button>` : ''}
          </td>
        </tr>`;
    }).join('');

    tbody.querySelectorAll('tr.connector-row').forEach((tr) => {
      const id = tr.dataset.id;
      tr.querySelector('.act-manage').addEventListener('click', () => this.#openDetail(id));
      tr.querySelector('.act-delete')?.addEventListener('click', () => this.#deleteConnector(id));
      tr.addEventListener('click', (e) => {
        if (!e.target.closest('button')) this.#openDetail(id);
      });
    });
  }

  async #deleteConnector(id) {
    const c = this.#connectors.find((x) => x.connector_id === id);
    if (!confirm(`Delete connector "${c?.display_name || c?.name || id}"? Agents will lose access to its tools.`)) return;
    try {
      await window.deleteMcpConnector(id);
      this.#load();
    } catch (e) {
      alert(`Delete failed: ${e.message}`);
    }
  }

  // ── Connector detail modal ───────────────────────────────────────────────

  async #openDetail(id) {
    const c = this.#connectors.find((x) => x.connector_id === id);
    if (!c) return;
    const modal = this.querySelector('#detail-modal');
    modal.setAttribute('heading', c.display_name || c.name);
    const body = this.querySelector('#detail-body');
    const st = this.#connectorStatus(c);
    body.innerHTML = `
      <div class="detail-meta">
        ${this.#metaRow('URL', c.url || '—')}
        ${this.#metaRow('Transport', c.transport || '—')}
        ${this.#metaRow('Auth type', AUTH_LABELS[c.auth_type] || c.auth_type || '—')}
        ${this.#metaRow('Tools', String(c.tool_count ?? 0))}
        ${this.#metaRow('Status', st.label)}
        ${c.description ? this.#metaRow('Description', c.description) : ''}
      </div>
      <div id="detail-auth-section">${c.auth_type && c.auth_type !== 'none' ? '<div class="detail-loading">Checking connection…</div>' : ''}</div>
    `;
    modal.open();
    if (c.auth_type === 'oauth2') this.#renderOauthSection(c);
    else if (c.auth_type && c.auth_type !== 'none') this.#renderCredentialSection(c);
  }

  #metaRow(label, value) {
    return `<div class="meta-row"><span class="meta-label">${this.#esc(label)}</span><span class="meta-value">${this.#esc(value)}</span></div>`;
  }

  async #renderCredentialSection(c) {
    const section = this.querySelector('#detail-auth-section');
    let connected = false;
    try {
      const resp = await window.fetchMcpCredentialStatus(c.connector_id);
      connected = !!resp?.data?.connected;
    } catch { /* leave disconnected */ }
    section.innerHTML = `
      <h4 class="detail-subtitle">${icons.key('', 14)} Credential</h4>
      <div class="cred-status">
        <span class="status"><span class="status-dot ${connected ? 'is-ok' : 'is-off'}"></span>${connected ? 'Credential set' : 'No credential set'}</span>
        ${connected ? `<button class="btn-ghost danger" id="cred-remove" type="button">Remove</button>` : ''}
      </div>
      <div class="cred-form">
        <input type="password" id="cred-value" placeholder="${c.auth_type === 'basic' ? 'username:password' : 'API key / token'}" />
        <button class="btn-dark" id="cred-save" type="button">${connected ? 'Replace' : 'Save'}</button>
      </div>
      <div class="form-error" id="cred-error" hidden></div>
    `;
    section.querySelector('#cred-save').addEventListener('click', async () => {
      const value = section.querySelector('#cred-value').value.trim();
      if (!value) return;
      const err = section.querySelector('#cred-error');
      err.hidden = true;
      try {
        const resp = await window.setMcpCredential(c.connector_id, value);
        if (resp?.data?.connected === false) {
          err.textContent = `Stored, but verification failed: ${resp.data.error || 'unknown error'}`;
          err.hidden = false;
        }
        this.#renderCredentialSection(c);
      } catch (e) {
        err.textContent = `Failed to save credential: ${e.message}`;
        err.hidden = false;
      }
    });
    section.querySelector('#cred-remove')?.addEventListener('click', async () => {
      try {
        await window.deleteMcpCredential(c.connector_id);
        this.#renderCredentialSection(c);
      } catch (e) {
        alert(`Remove failed: ${e.message}`);
      }
    });
  }

  async #renderOauthSection(c) {
    const section = this.querySelector('#detail-auth-section');
    let status = { authorized: false, expires_at: null };
    try {
      const resp = await window.fetchMcpOauthStatus(c.connector_id);
      status = resp?.data ?? status;
    } catch { /* leave unauthorized */ }
    const expiry = status.expires_at
      ? ` · expires ${new Date(status.expires_at).toLocaleString()}` : '';
    section.innerHTML = `
      <h4 class="detail-subtitle">${icons.shield('', 14)} OAuth 2.1</h4>
      <div class="cred-status">
        <span class="status"><span class="status-dot ${status.authorized ? 'is-ok' : 'is-off'}"></span>${status.authorized ? `Authorized${this.#esc(expiry)}` : 'Not authorized'}</span>
        <span class="cred-actions">
          ${status.authorized
            ? `<button class="btn-ghost danger" id="oauth-revoke" type="button">Revoke</button>`
            : `<button class="btn-dark" id="oauth-authorize" type="button">${icons.externalLink('', 14)} Authorize</button>`}
        </span>
      </div>
      <div class="form-error" id="oauth-error" hidden></div>
    `;
    section.querySelector('#oauth-authorize')?.addEventListener('click', async () => {
      const err = section.querySelector('#oauth-error');
      err.hidden = true;
      try {
        const resp = await window.authorizeMcpOauth(c.connector_id);
        const url = resp?.data?.authorization_url;
        if (url) window.open(url, 'mcp-oauth', 'width=600,height=720');
      } catch (e) {
        err.textContent = `Authorization failed: ${e.message}`;
        err.hidden = false;
      }
    });
    section.querySelector('#oauth-revoke')?.addEventListener('click', async () => {
      try {
        await window.revokeMcpOauthToken(c.connector_id);
        this.#renderOauthSection(c);
      } catch (e) {
        alert(`Revoke failed: ${e.message}`);
      }
    });
  }

  // ── Register connector modal ─────────────────────────────────────────────

  #registerModalHtml() {
    return `
      <app-modal heading="Register connector" id="register-modal">
        <form id="register-form" class="modal-form">
          <label>Name
            <input name="name" required placeholder="github" autocomplete="off" />
          </label>
          <label>Display name
            <input name="display_name" placeholder="GitHub" autocomplete="off" />
          </label>
          <label>Server URL
            <div class="url-row">
              <input name="url" required type="url" placeholder="https://mcp.example.com/mcp" autocomplete="off" />
              <button class="btn-outline" id="probe-btn" type="button">${icons.search('', 14)} Probe</button>
            </div>
          </label>
          <div class="probe-result" id="probe-result" hidden></div>
          <label>Auth type
            <select name="auth_type" id="register-auth-type">
              <option value="none">No auth</option>
              <option value="bearer">API key (bearer)</option>
              <option value="basic">Basic auth</option>
              <option value="oauth2">OAuth 2.1</option>
              <option value="url_param">URL parameter</option>
            </select>
          </label>
          <div class="auth-fields" data-auth="bearer" hidden>
            <label>Credential header <span class="opt">(optional, default Authorization)</span>
              <input name="credential_header_name" placeholder="X-Api-Key" autocomplete="off" />
            </label>
          </div>
          <div class="auth-fields" data-auth="basic" hidden>
            <label>Username <input name="basic_username" autocomplete="off" /></label>
            <label>Password <input name="basic_password" type="password" autocomplete="off" /></label>
          </div>
          <div class="auth-fields" data-auth="oauth2" hidden>
            <label>OAuth client ID <span class="opt">(leave blank if the server supports DCR)</span>
              <input name="oauth_client_id" autocomplete="off" />
            </label>
            <label>OAuth client secret
              <input name="oauth_client_secret" type="password" autocomplete="off" />
            </label>
          </div>
          <div class="auth-fields" data-auth="url_param" hidden>
            <label>URL parameter name <input name="url_param_name" placeholder="api_key" autocomplete="off" /></label>
          </div>
          <label>Description
            <textarea name="description" rows="2" placeholder="What this server's tools do"></textarea>
          </label>
          <div class="form-error" id="register-error" hidden></div>
        </form>
        <div data-slot="footer">
          <button class="btn-outline" id="register-cancel" type="button">Cancel</button>
          <button class="btn-dark" id="register-submit" type="button">Register</button>
        </div>
      </app-modal>`;
  }

  #wireRegisterModal() {
    const modal = this.querySelector('#register-modal');
    const form = this.querySelector('#register-form');
    const authSelect = this.querySelector('#register-auth-type');
    const syncAuthFields = () => {
      modal.querySelectorAll('.auth-fields').forEach((el) => {
        el.hidden = el.dataset.auth !== authSelect.value;
      });
    };
    authSelect.addEventListener('change', syncAuthFields);

    this.querySelector('#probe-btn').addEventListener('click', async () => {
      const url = form.elements.url.value.trim();
      const result = this.querySelector('#probe-result');
      if (!url) return;
      result.hidden = false;
      result.className = 'probe-result';
      result.innerHTML = 'Probing…';
      try {
        const resp = await window.probeMcpConnector(url);
        const d = resp?.data ?? {};
        if (d.auth_type) {
          authSelect.value = d.auth_type;
          syncAuthFields();
        }
        result.className = 'probe-result is-ok';
        result.innerHTML = `${icons.checkCircle('', 14)} Detected auth: <strong>${this.#esc(AUTH_LABELS[d.auth_type] || d.auth_type)}</strong>${d.hint ? ` — ${this.#esc(d.hint)}` : ''}`;
      } catch (e) {
        result.className = 'probe-result is-error';
        result.innerHTML = `${icons.xCircle('', 14)} Probe failed: ${this.#esc(e.message)}`;
      }
    });

    this.querySelector('#register-cancel').addEventListener('click', () => modal.close());
    this.querySelector('#register-submit').addEventListener('click', async () => {
      if (!form.reportValidity()) return;
      const err = this.querySelector('#register-error');
      err.hidden = true;
      const f = form.elements;
      const body = { name: f.name.value.trim(), url: f.url.value.trim(), auth_type: f.auth_type.value };
      for (const key of ['display_name', 'description', 'credential_header_name',
        'basic_username', 'basic_password', 'oauth_client_id', 'oauth_client_secret', 'url_param_name']) {
        const v = f[key].value.trim();
        if (v) body[key] = v;
      }
      try {
        await window.registerMcpConnector(body);
        modal.close();
        form.reset();
        this.querySelector('#probe-result').hidden = true;
        this.#load();
      } catch (e) {
        err.textContent = `Failed to register: ${e.message}`;
        err.hidden = false;
      }
    });
  }

  #openRegister() {
    this.querySelector('#register-modal').open();
  }

  // ── Upload MCP server modal ──────────────────────────────────────────────

  #uploadModalHtml() {
    return `
      <app-modal heading="Upload MCP server" id="upload-modal">
        <div class="upload-tabs" role="tablist">
          <button class="upload-tab is-active" data-tab="zip" type="button" role="tab">${icons.upload('', 14)} Upload zip</button>
          <button class="upload-tab" data-tab="github" type="button" role="tab">${icons.github('', 14)} From GitHub</button>
        </div>
        <form id="upload-zip-form" class="modal-form" data-pane="zip">
          <label>Name <input name="name" required placeholder="my-mcp-server" autocomplete="off" /></label>
          <label>Version tag <input name="version_tag" placeholder="v1" autocomplete="off" /></label>
          <label>Source archive (.zip)
            <input name="file" type="file" accept=".zip,application/zip" required />
          </label>
        </form>
        <form id="upload-github-form" class="modal-form" data-pane="github" hidden>
          <label>Name <input name="name" required placeholder="my-mcp-server" autocomplete="off" /></label>
          <label>Version tag <input name="version_tag" placeholder="v1" autocomplete="off" /></label>
          <label>GitHub repository URL
            <input name="github_url" required type="url" placeholder="https://github.com/org/repo" autocomplete="off" />
          </label>
        </form>
        <div class="form-error" id="upload-error" hidden></div>
        <div class="upload-queued" id="upload-queued" hidden></div>
        <div data-slot="footer">
          <button class="btn-outline" id="upload-cancel" type="button">Cancel</button>
          <button class="btn-dark" id="upload-submit" type="button">Queue build</button>
        </div>
      </app-modal>`;
  }

  #wireUploadModal() {
    const modal = this.querySelector('#upload-modal');
    let activeTab = 'zip';
    modal.querySelectorAll('.upload-tab').forEach((tab) => {
      tab.addEventListener('click', () => {
        activeTab = tab.dataset.tab;
        modal.querySelectorAll('.upload-tab').forEach((t) =>
          t.classList.toggle('is-active', t === tab));
        modal.querySelectorAll('[data-pane]').forEach((p) => {
          p.hidden = p.dataset.pane !== activeTab;
        });
      });
    });
    this.querySelector('#upload-cancel').addEventListener('click', () => modal.close());
    this.querySelector('#upload-submit').addEventListener('click', async () => {
      const err = this.querySelector('#upload-error');
      const queued = this.querySelector('#upload-queued');
      err.hidden = true;
      queued.hidden = true;
      try {
        let resp;
        if (activeTab === 'zip') {
          const form = this.querySelector('#upload-zip-form');
          if (!form.reportValidity()) return;
          const fd = new FormData();
          fd.append('name', form.elements.name.value.trim());
          fd.append('version_tag', form.elements.version_tag.value.trim() || 'v1');
          fd.append('file', form.elements.file.files[0]);
          resp = await window.uploadMcpServerZip(fd);
        } else {
          const form = this.querySelector('#upload-github-form');
          if (!form.reportValidity()) return;
          resp = await window.uploadMcpServerGithub({
            name: form.elements.name.value.trim(),
            version_tag: form.elements.version_tag.value.trim() || 'v1',
            github_url: form.elements.github_url.value.trim(),
          });
        }
        queued.innerHTML = `${icons.checkCircle('', 14)} Build queued — connector <code>${this.#esc(resp?.data?.connector_id || '')}</code>. Track progress under “My uploads”.`;
        queued.hidden = false;
        this.#load();
      } catch (e) {
        err.textContent = `Upload failed: ${e.message}`;
        err.hidden = false;
      }
    });
  }

  #openUpload() {
    this.querySelector('#upload-modal').open();
  }

  // ── My uploads ───────────────────────────────────────────────────────────

  #renderUploads() {
    const section = this.querySelector('#uploads-section');
    if (!this.#uploads.length) {
      section.hidden = true;
      return;
    }
    section.hidden = false;
    const chipClass = { Active: 'is-ok', Deploying: 'is-building', Failed: 'is-error' };
    const tbody = this.querySelector('#uploads-tbody');
    tbody.innerHTML = this.#uploads.map((u) => {
      const info = u.upload_info || {};
      const cls = chipClass[info.upload_status] || 'is-off';
      return `
        <tr data-id="${this.#esc(u.connector_id)}" data-name="${this.#esc(u.connector_name)}">
          <td class="cell-name"><span class="name-main">${this.#esc(u.connector_name)}</span></td>
          <td><span class="chip ${cls}">${this.#esc(info.upload_status || 'Unknown')}</span></td>
          <td class="cell-muted">${this.#esc(info.error_detail || info.status_message || '—')}</td>
          <td class="cell-url">${this.#esc(u.url || '—')}</td>
          <td class="cell-actions">
            <button class="btn-ghost act-logs" type="button" title="Build logs">${icons.terminal('', 14)}</button>
          </td>
        </tr>`;
    }).join('');
    tbody.querySelectorAll('.act-logs').forEach((btn) => {
      btn.addEventListener('click', () => {
        const tr = btn.closest('tr');
        this.#showBuildLogs(tr.dataset.id, tr.dataset.name);
      });
    });
  }

  async #showBuildLogs(connectorId, name) {
    const panel = this.querySelector('#logs-panel');
    panel.hidden = false;
    panel.scrollIntoView({ block: 'nearest' });
    this.querySelector('#logs-name').textContent = name;
    const pre = this.querySelector('#logs-pre');
    pre.textContent = 'Loading logs…';
    try {
      const resp = await window.fetchMcpBuildLogs(connectorId, 200);
      const logs = typeof resp?.data === 'string' ? resp.data : (resp?.data ?? '');
      pre.textContent = logs || '(no logs)';
    } catch (e) {
      pre.textContent = `Failed to load logs: ${e.message}`;
    }
  }

  // ── Agent access ─────────────────────────────────────────────────────────

  async #loadAgentAccess() {
    const body = this.querySelector('#agent-access-body');
    if (!this.#selectedAgentId) {
      body.innerHTML = `<div class="agent-access-empty">${icons.network('', 28)}<p>Select an agent to manage its connector access</p></div>`;
      return;
    }
    body.innerHTML = `<div class="detail-loading" aria-busy="true"><app-skeleton lines="3"></app-skeleton></div>`;
    try {
      const resp = await window.fetchAgentMcpConnectors(this.#selectedAgentId);
      this.#agentConnectors = resp?.data?.connectors || [];
    } catch (e) {
      body.innerHTML = `<div class="agent-access-empty"><p>Failed to load connector access: ${this.#esc(e.message)}</p></div>`;
      return;
    }
    this.#agentTools = new Map();
    this.#renderAgentAccess();
  }

  #renderAgentAccess() {
    const body = this.querySelector('#agent-access-body');
    if (!this.#agentConnectors.length) {
      body.innerHTML = `<div class="agent-access-empty">${icons.cube('', 28)}<p>No connectors are available to this agent yet</p></div>`;
      return;
    }
    body.innerHTML = `
      <table class="agent-access-table">
        <thead><tr><th>Connector</th><th>Description</th><th>Enabled</th><th class="th-actions"></th></tr></thead>
        <tbody>
          ${this.#agentConnectors.map((c) => `
            <tr data-id="${this.#esc(c.connector_id)}">
              <td class="cell-name"><span class="name-main">${this.#esc(c.display_name || c.name)}</span></td>
              <td class="cell-muted">${this.#esc(c.description || '—')}</td>
              <td>
                <label class="switch">
                  <input type="checkbox" class="access-toggle" ${c.enabled ? 'checked' : ''} />
                  <span class="slider"></span>
                </label>
              </td>
              <td class="cell-actions">
                <button class="btn-ghost act-tools" type="button">${icons.chevronDown('', 14)} Tools</button>
              </td>
            </tr>
            <tr class="tools-row" data-for="${this.#esc(c.connector_id)}" hidden>
              <td colspan="4"><div class="tools-editor" data-id="${this.#esc(c.connector_id)}"></div></td>
            </tr>
          `).join('')}
        </tbody>
      </table>`;

    body.querySelectorAll('.access-toggle').forEach((toggle) => {
      toggle.addEventListener('change', async () => {
        const id = toggle.closest('tr').dataset.id;
        try {
          await window.setAgentMcpConnectorAccess(this.#selectedAgentId, id, toggle.checked);
        } catch (e) {
          toggle.checked = !toggle.checked;
          alert(`Failed to update access: ${e.message}`);
        }
      });
    });
    body.querySelectorAll('.act-tools').forEach((btn) => {
      btn.addEventListener('click', () => {
        const id = btn.closest('tr').dataset.id;
        const row = body.querySelector(`.tools-row[data-for="${CSS.escape(id)}"]`);
        row.hidden = !row.hidden;
        if (!row.hidden) this.#renderToolsEditor(id);
      });
    });
  }

  async #renderToolsEditor(connectorId) {
    const editor = this.querySelector(`.tools-editor[data-id="${CSS.escape(connectorId)}"]`);
    if (!editor) return;
    if (!this.#agentTools.has(connectorId)) {
      editor.innerHTML = `<div class="detail-loading" aria-busy="true"><app-skeleton lines="3"></app-skeleton></div>`;
      try {
        const resp = await window.fetchAgentMcpConnectorTools(this.#selectedAgentId, connectorId);
        this.#agentTools.set(connectorId, resp?.data?.tools || []);
      } catch (e) {
        editor.innerHTML = `<div class="form-error">Failed to load tools: ${this.#esc(e.message)}</div>`;
        return;
      }
    }
    const tools = this.#agentTools.get(connectorId);
    if (!tools.length) {
      editor.innerHTML = `<div class="cell-muted tools-empty">This connector exposes no tools yet.</div>`;
      return;
    }
    editor.innerHTML = `
      <div class="tools-list">
        ${tools.map((t, i) => `
          <div class="tool-line">
            <div class="tool-info">
              <span class="tool-name">${this.#esc(t.name)}</span>
              ${t.description ? `<span class="tool-desc">${this.#esc(t.description)}</span>` : ''}
            </div>
            <select class="tool-stance" data-index="${i}">
              <option value="allow" ${t.stance !== 'deny' ? 'selected' : ''}>Allow</option>
              <option value="deny" ${t.stance === 'deny' ? 'selected' : ''}>Deny</option>
            </select>
          </div>`).join('')}
      </div>
      <div class="tools-actions">
        <span class="tools-save-status" hidden></span>
        <button class="btn-dark tools-save" type="button">Save rules</button>
      </div>`;
    editor.querySelector('.tools-save').addEventListener('click', async () => {
      const rules = [...editor.querySelectorAll('.tool-stance')].map((sel) => ({
        connector_id: connectorId,
        tool_pattern: tools[Number(sel.dataset.index)].name,
        stance: sel.value,
      }));
      const statusEl = editor.querySelector('.tools-save-status');
      try {
        await window.saveAgentMcpToolRules(this.#selectedAgentId, rules);
        statusEl.textContent = 'Saved';
        statusEl.className = 'tools-save-status is-ok';
      } catch (e) {
        statusEl.textContent = `Save failed: ${e.message}`;
        statusEl.className = 'tools-save-status is-error';
      }
      statusEl.hidden = false;
    });
  }

  #esc(str) {
    if (!str) return '';
    return String(str).replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('mcp-page', McpPage);
