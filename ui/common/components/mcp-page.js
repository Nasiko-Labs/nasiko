/**
 * MCP gateway — one unified catalog of connectable services (platform
 * Composio toolkits ∪ custom MCP servers), upload-your-own servers, and
 * per-agent tool access.
 *
 * @element mcp-page
 * @note Data sources (all under /api/mcp, envelope {data, status_code, message}):
 *       `window.fetchMcpConnectors()` → GET /api/mcp/connectors
 *         → data = {created_by_you: [dto], shared_with_you: [dto], total}
 *       `window.fetchMcpToolkits()` → GET /api/mcp/composio/toolkits
 *         → data = {toolkits: [{connector_id, name, display_name, description,
 *           logo_url, auth_flow, tool_count, is_connected}], total}
 *       The catalog grid merges those two client-side rather than using
 *       GET /api/mcp/catalog: the catalog view has no is_connected, ownership,
 *       version, or owner_username, all of which the cards and tabs need.
 *       `window.fetchMcpMyUploads()` → GET /api/mcp/connectors/my-uploads
 *       plus register/probe/update/delete, credential + OAuth management,
 *       connect/disconnect (`/mcp/connect`, `/mcp/connections`),
 *       upload (zip/GitHub) + build status/logs, and the per-agent
 *       connector/tool-rule endpoints — see oss/ui/web/navigation.js.
 */
import styles from './mcp-page.css' with { type: 'css' };
import { icons } from '../utils/icons.js';
import { attachSlidingIndicator } from '../utils/tab-indicator.js';
import { confirmDialog } from '../utils/confirm-dialog.js';
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

// auth_type → connect flow for custom servers; mirrors `auth_flow_for` in
// oss/mcp-gateway/src/catalog.rs (toolkits carry auth_flow from the API).
const AUTH_FLOWS = { oauth2: 'oauth', bearer: 'api_key', basic: 'api_key', url_param: 'api_key' };

// Nav scopes filter the unified catalog grid. Ownership scopes apply to
// custom MCP servers only — Composio toolkits are platform-registered.
const CATALOG_SCOPES = {
  all: { filter: () => true, empty: '' },
  'my-servers': {
    filter: (s) => s.kind === 'server' && !s.__shared,
    empty: 'You haven\'t registered or uploaded any MCP servers yet',
  },
  'shared-with-me': {
    filter: (s) => s.kind === 'server' && s.__shared,
    empty: 'No MCP servers have been shared with you yet',
  },
  toolkits: {
    filter: (s) => s.kind === 'toolkit',
    empty: 'Platform-registered app toolkits (Gmail, Notion, …) show up here once an admin adds them',
  },
};

const CATALOG_TABS = [['all', 'All'], ['available', 'Available to connect'], ['connected', 'Connected']];

class McpPage extends HTMLElement {
  #initialized = false;
  #connectors = [];
  #toolkits = [];
  #agents = [];
  #selectedAgentId = '';
  #agentConnectors = [];
  #agentTools = new Map(); // connector_id → [{name, description, stance}]
  #catalogScope = 'all'; // key into CATALOG_SCOPES
  #catalogTab = 'all'; // all | available | connected
  #connectTargetId = ''; // service awaiting an API key in the connect modal

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <app-module-nav module="mcp"></app-module-nav>
      <div class="page-head">
        <div>
          <h1 class="title-page">MCP gateway</h1>
          <p class="page-sub">Connect external MCP servers and control which agents can use their tools</p>
        </div>
        <div class="head-actions">
          <button class="btn-outline" id="upload-btn" type="button">${icons.upload('', 14)} Upload MCP server</button>
          <button class="btn-dark" id="register-btn" type="button">${icons.plus('', 14)} Register connector</button>
        </div>
      </div>

      <div id="catalog-section">
        <div class="tk-tabs" id="catalog-tabs" role="tablist" aria-hidden="true">
          <app-skeleton class="tk-skel-tab" height="0.9rem"></app-skeleton>
          <app-skeleton class="tk-skel-tab" height="0.9rem"></app-skeleton>
          <app-skeleton class="tk-skel-tab" height="0.9rem"></app-skeleton>
        </div>
        <div class="tk-grid" id="catalog-grid" aria-busy="true">${this.#catalogSkeletonCards()}</div>
      </div>

      ${this.#registerModalHtml()}
      ${this.#uploadModalHtml()}
      ${this.#connectModalHtml()}
      <app-modal heading="Connector" id="detail-modal" hide-footer>
        <div id="detail-body"></div>
      </app-modal>
    `;

    // Module tree nav: catalog scopes re-filter the unified grid. The "My
    // servers" scope also reveals the uploads table below the catalog.
    this.addEventListener('module-nav-select', (e) => {
      const section = e.detail.section;
      if (CATALOG_SCOPES[section]) {
        this.#catalogScope = section;
        this.#catalogTab = 'all';
        this.#renderCatalog();
        // The catalog is the first section — return to the top so the page
        // head and tab strip stay in view alongside the filtered grid.
        window.scrollTo({ top: 0, behavior: 'smooth' });
        return;
      }
    });

    this.querySelector('#register-btn').addEventListener('click', () => this.#openRegister());
    this.querySelector('#upload-btn').addEventListener('click', () => this.#openUpload());
    this.#wireRegisterModal();
    this.#wireUploadModal();
    this.#wireConnectModal();
    // Tab clicks are delegated — the tab strip re-renders after data loads.
    attachSlidingIndicator(this.querySelector('#catalog-tabs'), '.tk-tab', '.active');
    this.querySelector('#catalog-tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.tk-tab');
      if (!tab) return;
      this.#catalogTab = tab.dataset.tab;
      this.#renderCatalog();
    });

    this.#load();
    this.#loadAgents();
  }

  // ── Data loading ─────────────────────────────────────────────────────────

  async #load() {
    let connResp;
    let toolkitsResp;
    try {
      [connResp, toolkitsResp] = await Promise.all([
        window.fetchMcpConnectors(),
        window.fetchMcpToolkits().catch(() => ({ data: { toolkits: [] } })),
      ]);
    } catch (e) {
      console.error('MCP catalog fetch failed:', e);
      const tabs = this.querySelector('#catalog-tabs');
      tabs.hidden = true;
      tabs.innerHTML = '';
      const grid = this.querySelector('#catalog-grid');
      grid.removeAttribute('aria-busy');
      grid.innerHTML = `<div class="tk-msg">Failed to load connectable services</div>`;
      return;
    }
    const d = connResp?.data ?? {};
    this.#connectors = [
      ...(d.created_by_you || []),
      ...(d.shared_with_you || []).map((c) => ({ ...c, __shared: true })),
    ];
    this.#toolkits = toolkitsResp?.data?.toolkits || [];
    this.#renderCatalog();
    this.#scheduleBuildPoll();
  }

  async #loadAgents() {
    try {
      const resp = await window.fetchAgents('', 1, 100);
      this.#agents = resp?.data || [];
    } catch {
      this.#agents = [];
    }
  }

  // ── Unified catalog (Composio toolkits ∪ custom MCP servers) ────────────

  /** Both kinds normalized into one card model, sorted by display name. */
  #services() {
    const toolkits = this.#toolkits.map((t) => ({ ...t, kind: 'toolkit' }));
    const servers = this.#connectors.map((c) => ({
      ...c,
      kind: 'server',
      auth_flow: AUTH_FLOWS[c.auth_type] || 'none',
    }));
    return [...toolkits, ...servers].sort((a, b) =>
      (a.display_name || a.name).localeCompare(b.display_name || b.name));
  }

  #findService(id) {
    return this.#services().find((s) => s.connector_id === id);
  }

  #catalogSkeletonCards() {
    return Array.from({ length: 6 }, () => `
      <div class="tk-card tk-card-skel" aria-hidden="true">
        <div class="tk-top">
          <app-skeleton class="tk-skel-logo" height="32px"></app-skeleton>
          <div class="tk-id">
            <app-skeleton class="tk-skel-name" height="20px"></app-skeleton>
            <app-skeleton class="tk-skel-chip" height="20px"></app-skeleton>
          </div>
          <app-skeleton class="tk-skel-btn" height="28px"></app-skeleton>
        </div>
        <app-skeleton lines="2"></app-skeleton>
      </div>`).join('');
  }

  #renderCatalog() {
    const tabs = this.querySelector('#catalog-tabs');
    const grid = this.querySelector('#catalog-grid');
    grid.removeAttribute('aria-busy');
    const scoped = this.#services().filter(CATALOG_SCOPES[this.#catalogScope].filter);

    if (!scoped.length) {
      tabs.hidden = true;
      tabs.innerHTML = '';
      grid.innerHTML = this.#catalogEmptyHtml();
      grid.querySelector('#empty-register-btn')
        ?.addEventListener('click', () => this.#openRegister());
      return;
    }

    const connected = scoped.filter((s) => s.is_connected);
    const counts = {
      all: scoped.length,
      available: scoped.length - connected.length,
      connected: connected.length,
    };
    tabs.hidden = false;
    tabs.removeAttribute('aria-hidden');
    tabs.innerHTML = CATALOG_TABS.map(([key, label]) => `
      <button class="tk-tab ${this.#catalogTab === key ? 'active' : ''}" type="button"
              role="tab" aria-selected="${this.#catalogTab === key}" data-tab="${key}">
        ${label}<span class="n">(${counts[key]})</span>
      </button>`).join('');

    const visible = {
      all: scoped,
      available: scoped.filter((s) => !s.is_connected),
      connected,
    }[this.#catalogTab] || scoped;
    grid.innerHTML = visible.length
      ? visible.map((s) => this.#serviceCardHtml(s)).join('')
      : `<div class="tk-msg">${this.#catalogTab === 'connected'
        ? 'Nothing connected yet'
        : 'Everything here is already connected'}</div>`;

    grid.querySelectorAll('.tk-card').forEach((card) => {
      const id = card.dataset.id;
      card.querySelector('.act-connect')
        ?.addEventListener('click', () => this.#connectService(id));
      card.querySelector('.act-disconnect')
        ?.addEventListener('click', () => this.#disconnectService(id));
      // Broken logo URLs fall back to the letter avatar underneath.
      card.querySelector('.tk-logo img')
        ?.addEventListener('error', (e) => e.target.remove());
      // Custom server cards navigate to the full detail page.
      if (card.classList.contains('is-clickable')) {
        card.addEventListener('click', (e) => {
          if (!e.target.closest('button')) {
            window.location.href = '/mcp-detail.html?id=' + encodeURIComponent(id);
          }
        });
      }
    });
  }

  #catalogEmptyHtml() {
    if (this.#catalogScope !== 'all') {
      return `<div class="tk-msg">${this.#esc(CATALOG_SCOPES[this.#catalogScope].empty)}</div>`;
    }
    return `
      <div class="empty-state">
        ${icons.network('', 32)}
        <h3>No connectable services yet</h3>
        <p>Register an external MCP server or upload your own to give agents new tools.</p>
        <button class="btn-dark" id="empty-register-btn" type="button">${icons.plus('', 14)} Register connector</button>
      </div>`;
  }

  #serviceCardHtml(s) {
    const name = s.display_name || s.name;
    const isSettingUp = s.kind === 'server' && s.source_kind === 'uploaded_build'
      && (s.build_status === 'pending' || s.build_status === 'building');
    const isFailed = s.kind === 'server' && s.source_kind === 'uploaded_build'
      && s.build_status === 'failed';
    const chips = [`<span class="tk-chip">${s.tool_count ?? 0} tools</span>`];
    if (s.version) chips.push(`<span class="tk-chip">${this.#esc(s.version)}</span>`);
    if (s.__shared) {
      chips.push(`<span class="tk-by">shared by ${this.#esc(s.owner_username || 'someone')}</span>`);
    }
    const cardCls = isSettingUp ? ' tk-card--setting-up' : isFailed ? ' tk-card--failed' : '';
    let bodyHtml;
    if (isSettingUp) {
      bodyHtml = `<div class="tk-setup"><span class="setup-spinner"></span><span class="tk-setup-label">Building and deploying...</span></div>`;
    } else if (isFailed) {
      bodyHtml = `<div class="tk-setup tk-setup--error">${icons.xCircle('', 14)}<span class="tk-setup-label">Build failed</span></div>`;
    } else {
      bodyHtml = `<p class="tk-desc">${this.#esc(s.description || 'No description provided.')}</p>`;
    }
    return `
      <div class="tk-card${s.kind === 'server' ? ' is-clickable' : ''}${cardCls}" data-id="${this.#esc(s.connector_id)}">
        <div class="tk-top">
          <span class="tk-logo" aria-hidden="true">${this.#esc(name.charAt(0))}${s.logo_url
            ? `<img src="${this.#esc(s.logo_url)}" alt="" loading="lazy" />` : ''}</span>
          <div class="tk-id">
            <span class="tk-name" title="${this.#esc(name)}">${this.#esc(name)}</span>
            <span class="tk-chips">${chips.join('')}</span>
          </div>
          ${isSettingUp || isFailed ? '' : this.#serviceActionHtml(s, name)}
        </div>
        ${bodyHtml}
      </div>`;
  }

  #serviceActionHtml(s, name) {
    if (s.is_connected) {
      return `<button class="tk-action is-connected act-disconnect" type="button"
                title="Disconnect ${this.#esc(name)}" aria-label="Disconnect ${this.#esc(name)}">
          <span class="tk-rest">${icons.check('', 14)} Connected</span>
          <span class="tk-hover">${icons.x('', 14)} Disconnect</span>
        </button>`;
    }
    return `<button class="tk-action tk-action-ghost act-connect" type="button" title="Connect ${this.#esc(name)}">${icons.plus('', 14)} Connect</button>`;
  }

  // ── Connect / disconnect (shared by toolkits and custom servers) ────────

  async #connectService(id) {
    const s = this.#findService(id);
    if (!s) return;
    if (s.auth_flow === 'api_key') {
      this.#openConnectModal(s);
      return;
    }
    try {
      const resp = await window.connectMcpService({ connector_id: id });
      this.#applyConnectOutcome(resp?.data);
    } catch (e) {
      alert(`Connect failed: ${e.message}`);
    }
  }

  /** Connected → refresh; OAuth URL → popup, poll until it closes then reload. */
  #applyConnectOutcome(d) {
    const url = d?.oauth_url || d?.authorization_url;
    if (url) {
      const popup = window.open(url, 'mcp-oauth', 'width=600,height=720');
      if (popup) {
        const poll = setInterval(() => {
          if (popup.closed) {
            clearInterval(poll);
            this.#load();
          }
        }, 500);
      } else {
        // Popup blocked — fall back to focus listener
        window.addEventListener('focus', () => this.#load(), { once: true });
      }
      return;
    }
    this.#load();
  }

  async #disconnectService(id) {
    const s = this.#findService(id);
    const name = s?.display_name || s?.name || id;
    const confirmed = await confirmDialog({
      title: 'Disconnect ' + name,
      message: 'Agents lose access to its tools until you reconnect.',
      confirmLabel: 'Disconnect',
      danger: true,
    });
    if (!confirmed) return;
    try {
      await window.disconnectMcpConnection(id);
      this.#load();
    } catch (e) {
      alert(`Disconnect failed: ${e.message}`);
    }
  }

  #connectModalHtml() {
    return `
      <app-modal heading="Connect" id="connect-modal">
        <div class="modal-form">
          <label>API key / token
            <input type="password" id="connect-cred-value" placeholder="API key / token" autocomplete="off" />
          </label>
          <div class="form-error" id="connect-error" hidden></div>
        </div>
        <div data-slot="footer">
          <button class="btn-outline" id="connect-cancel" type="button">Cancel</button>
          <button class="btn-dark" id="connect-submit" type="button">Connect</button>
        </div>
      </app-modal>`;
  }

  #wireConnectModal() {
    const modal = this.querySelector('#connect-modal');
    this.querySelector('#connect-cancel').addEventListener('click', () => modal.close());
    this.querySelector('#connect-submit').addEventListener('click', async () => {
      const value = this.querySelector('#connect-cred-value').value.trim();
      if (!value) return;
      const err = this.querySelector('#connect-error');
      err.hidden = true;
      try {
        const resp = await window.connectMcpService({
          connector_id: this.#connectTargetId,
          credentials: { value },
        });
        modal.close();
        this.querySelector('#connect-cred-value').value = '';
        this.#applyConnectOutcome(resp?.data);
      } catch (e) {
        err.textContent = `Connect failed: ${e.message}`;
        err.hidden = false;
      }
    });
  }

  #openConnectModal(s) {
    this.#connectTargetId = s.connector_id;
    const modal = this.querySelector('#connect-modal');
    modal.setAttribute('heading', `Connect ${s.display_name || s.name}`);
    this.querySelector('#connect-cred-value').placeholder =
      s.auth_type === 'basic' ? 'username:password' : 'API key / token';
    modal.open();
  }

  async #deleteConnector(id) {
    const c = this.#connectors.find((x) => x.connector_id === id);
    const confirmed = await confirmDialog({
      title: 'Delete ' + (c?.display_name || c?.name || 'connector'),
      message: 'Agents will lose access to its tools. This cannot be undone.',
      confirmLabel: 'Delete',
      danger: true,
    });
    if (!confirmed) return false;
    try {
      await window.deleteMcpConnector(id);
      this.#load();
      return true;
    } catch (e) {
      alert(`Delete failed: ${e.message}`);
      return false;
    }
  }

  // ── Connector detail modal (custom servers) ──────────────────────────────

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

  async #openDetail(id) {
    const c = this.#connectors.find((x) => x.connector_id === id);
    if (!c) return;
    this.#selectedAgentId = '';
    this.#agentConnectors = [];
    this.#agentTools = new Map();
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
        ${c.version ? this.#metaRow('Version', c.version) : ''}
        ${c.__shared ? this.#metaRow('Shared by', c.owner_username || 'someone') : ''}
        ${c.description ? this.#metaRow('Description', c.description) : ''}
      </div>
      <div id="detail-auth-section">${c.auth_type && c.auth_type !== 'none' ? '<div class="detail-loading">Checking connection…</div>' : ''}</div>
      ${c.is_owner ? `
        <div class="detail-actions">
          <button class="btn-ghost danger" id="detail-delete" type="button">${icons.trash('', 14)} Delete connector</button>
        </div>` : ''}
      <h4 class="detail-subtitle">${icons.network('', 14)} Agent access</h4>
      <div class="agent-access-card">
        <div class="agent-picker">
          <label for="detail-agent-select">Agent</label>
          <auto-complete id="detail-agent-select" placeholder="Search agents…" aria-label="Agent"></auto-complete>
          <span class="agent-picker-hint">Select an agent to manage its access to this connector.</span>
        </div>
        <div id="detail-agent-access-body">
          <div class="agent-access-empty">${icons.network('', 28)}<p>Select an agent to manage its access to this connector</p></div>
        </div>
      </div>
    `;
    body.querySelector('#detail-delete')?.addEventListener('click', async () => {
      if (await this.#deleteConnector(id)) modal.close();
    });
    // Wire agent picker inside the detail modal.
    const agentPicker = body.querySelector('#detail-agent-select');
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
      this.#loadAgentAccessForConnector(id);
    });
    agentPicker.addEventListener('input', () => {
      if (agentPicker.value.trim() === '' && this.#selectedAgentId) {
        this.#selectedAgentId = '';
        const accessBody = body.querySelector('#detail-agent-access-body');
        accessBody.innerHTML = `<div class="agent-access-empty">${icons.network('', 28)}<p>Select an agent to manage its access to this connector</p></div>`;
      }
    });
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
        <div id="upload-picker" class="upload-picker">
          <button class="upload-method-card" data-method="zip" type="button">
            <span class="upload-method-icon">${icons.upload('', 22)}</span>
            <span class="upload-method-title">Upload a zip</span>
            <span class="upload-method-desc">Upload a .zip archive containing your MCP server source code</span>
          </button>
          <button class="upload-method-card" data-method="github" type="button">
            <span class="upload-method-icon">${icons.github('', 22)}</span>
            <span class="upload-method-title">Import from GitHub</span>
            <span class="upload-method-desc">Clone a GitHub repository containing your MCP server</span>
          </button>
        </div>
        <form id="upload-zip-form" class="modal-form" hidden>
          <label>Source archive (.zip)
            <input name="file" type="file" accept=".zip,application/zip" required />
          </label>
          <label>Name
            <input name="name" required placeholder="my-mcp-server" autocomplete="off" />
            <span class="field-hint">Auto-filled from the file name. You can change it.</span>
          </label>
          <label>Version tag <input name="version_tag" placeholder="v1" autocomplete="off" /></label>
        </form>
        <form id="upload-github-form" class="modal-form" hidden>
          <label>Name <input name="name" required placeholder="my-mcp-server" autocomplete="off" /></label>
          <label>Version tag <input name="version_tag" placeholder="v1" autocomplete="off" /></label>
          <label>GitHub repository URL
            <input name="github_url" required type="url" placeholder="https://github.com/org/repo" autocomplete="off" />
          </label>
        </form>
        <div class="form-error" id="upload-error" hidden></div>
        <div id="upload-footer" data-slot="footer" hidden>
          <button class="btn-outline" id="upload-back" type="button">${icons.chevronLeft('', 14)} Back</button>
          <button class="btn-dark" id="upload-submit" type="button">Upload and build</button>
        </div>
      </app-modal>`;
  }

  #wireUploadModal() {
    const modal = this.querySelector('#upload-modal');
    let activeMethod = '';
    const picker = this.querySelector('#upload-picker');
    const zipForm = this.querySelector('#upload-zip-form');
    const ghForm = this.querySelector('#upload-github-form');
    const footer = this.querySelector('#upload-footer');
    const err = this.querySelector('#upload-error');

    const showPicker = () => {
      activeMethod = '';
      picker.hidden = false;
      zipForm.hidden = true;
      ghForm.hidden = true;
      footer.hidden = true;
      err.hidden = true;
      modal.setAttribute('heading', 'Upload MCP server');
    };

    picker.querySelectorAll('.upload-method-card').forEach((card) => {
      card.addEventListener('click', () => {
        activeMethod = card.dataset.method;
        picker.hidden = true;
        zipForm.hidden = activeMethod !== 'zip';
        ghForm.hidden = activeMethod !== 'github';
        footer.hidden = false;
        modal.setAttribute('heading', activeMethod === 'zip' ? 'Upload zip' : 'Import from GitHub');
      });
    });

    this.querySelector('#upload-back').addEventListener('click', showPicker);

    // Auto-fill name from the chosen file, like the agent upload flow.
    zipForm.elements.file.addEventListener('change', () => {
      const file = zipForm.elements.file.files[0];
      if (!file || zipForm.elements.name.value.trim()) return;
      zipForm.elements.name.value = file.name
        .replace(/\.zip$/i, '')
        .trim()
        .toLowerCase()
        .replace(/[^a-z0-9._-]+/g, '-')
        .replace(/^[^a-z0-9_]+/, '')
        .replace(/-{2,}/g, '-')
        .replace(/-+$/, '')
        .slice(0, 128);
    });

    const submitBtn = this.querySelector('#upload-submit');
    submitBtn.addEventListener('click', async () => {
      err.hidden = true;
      submitBtn.disabled = true;
      submitBtn.textContent = 'Uploading...';
      try {
        let resp;
        if (activeMethod === 'zip') {
          if (!zipForm.reportValidity()) { submitBtn.disabled = false; submitBtn.textContent = 'Upload and build'; return; }
          const fd = new FormData();
          fd.append('name', zipForm.elements.name.value.trim());
          fd.append('version_tag', zipForm.elements.version_tag.value.trim() || 'v1');
          fd.append('file', zipForm.elements.file.files[0]);
          resp = await window.uploadMcpServerZip(fd);
        } else {
          if (!ghForm.reportValidity()) { submitBtn.disabled = false; submitBtn.textContent = 'Upload and build'; return; }
          resp = await window.uploadMcpServerGithub({
            name: ghForm.elements.name.value.trim(),
            version_tag: ghForm.elements.version_tag.value.trim() || 'v1',
            github_url: ghForm.elements.github_url.value.trim(),
          });
        }
        // Inject a placeholder card immediately so the user sees progress.
        const connName = (activeMethod === 'zip' ? zipForm.elements.name.value : ghForm.elements.name.value).trim();
        const connectorId = resp?.data?.connector_id || '';
        this.#connectors.push({
          connector_id: connectorId,
          name: connName,
          display_name: connName,
          kind: 'server',
          source_kind: 'uploaded_build',
          build_status: 'building',
          is_active: false,
          is_connected: false,
          is_owner: true,
          tool_count: 0,
        });
        this.#renderCatalog();
        modal.close();
        this.#scheduleBuildPoll();
      } catch (e) {
        err.textContent = `Upload failed: ${e.message}`;
        err.hidden = false;
      } finally {
        submitBtn.disabled = false;
        submitBtn.textContent = 'Upload and build';
      }
    });
  }

  /** Poll building connectors and re-render only the cards whose status changed. */
  #scheduleBuildPoll() {
    clearTimeout(this._pollTimer);
    const hasBuilding = this.#connectors.some(
      (c) => c.source_kind === 'uploaded_build'
        && (c.build_status === 'pending' || c.build_status === 'building'));
    if (hasBuilding) {
      this._pollTimer = setTimeout(() => this.#pollBuilding(), 5000);
    }
  }

  async #pollBuilding() {
    let connResp;
    try {
      connResp = await window.fetchMcpConnectors();
    } catch { this.#scheduleBuildPoll(); return; }
    const d = connResp?.data ?? {};
    const fresh = [
      ...(d.created_by_you || []),
      ...(d.shared_with_you || []).map((c) => ({ ...c, __shared: true })),
    ];
    const freshMap = new Map();
    for (const c of fresh) freshMap.set(c.connector_id, c);

    for (const c of this.#connectors) {
      if (c.source_kind !== 'uploaded_build') continue;
      if (c.build_status !== 'pending' && c.build_status !== 'building') continue;
      const fc = freshMap.get(c.connector_id);
      if (!fc || fc.build_status === c.build_status) continue;
      // Status changed — update in place and re-render just this card.
      const shared = c.__shared;
      Object.assign(c, fc);
      c.__shared = shared;
      // If build completed, fetch full detail to get tools/description.
      if (fc.build_status !== 'pending' && fc.build_status !== 'building') {
        try {
          const detail = await window.fetchMcpConnectorDetail(c.connector_id);
          const dd = detail?.data ?? detail;
          if (dd) {
            Object.assign(c, dd);
            c.__shared = shared;
          }
        } catch { /* best-effort */ }
      }
      this.#replaceCard(c);
    }
    this.#scheduleBuildPoll();
  }

  #replaceCard(c) {
    const card = this.querySelector(`.tk-card[data-id="${CSS.escape(c.connector_id)}"]`);
    if (!card) return;
    const tmp = document.createElement('div');
    tmp.innerHTML = this.#serviceCardHtml({ ...c, kind: 'server' });
    const newCard = tmp.firstElementChild;
    card.replaceWith(newCard);
    if (newCard.classList.contains('is-clickable')) {
      newCard.addEventListener('click', (e) => {
        if (!e.target.closest('button')) {
          window.location.href = '/mcp-detail.html?id=' + encodeURIComponent(c.connector_id);
        }
      });
    }
    newCard.querySelector('.act-connect')
      ?.addEventListener('click', () => this.#connectService(c.connector_id));
    newCard.querySelector('.act-disconnect')
      ?.addEventListener('click', () => this.#disconnectService(c.connector_id));
    newCard.querySelector('.tk-logo img')
      ?.addEventListener('error', (e) => e.target.remove());
  }

  #openUpload() {
    // Reset to the method picker step.
    this.querySelector('#upload-picker').hidden = false;
    this.querySelector('#upload-zip-form').hidden = true;
    this.querySelector('#upload-github-form').hidden = true;
    this.querySelector('#upload-footer').hidden = true;
    this.querySelector('#upload-error').hidden = true;
    this.querySelector('#upload-modal').setAttribute('heading', 'Upload MCP server');
    this.querySelector('#upload-modal').open();
  }

  // ── Agent access (per-connector, inside the detail modal) ────────────────

  /**
   * Load agent access for a specific connector and render it in the detail modal.
   * Shows whether the selected agent has access to this connector plus tool rules.
   */
  async #loadAgentAccessForConnector(connectorId) {
    const body = this.querySelector('#detail-agent-access-body');
    if (!this.#selectedAgentId) {
      body.innerHTML = `<div class="agent-access-empty">${icons.network('', 28)}<p>Select an agent to manage its access to this connector</p></div>`;
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
    // Find this specific connector in the agent's connector list.
    const match = this.#agentConnectors.find((c) => c.connector_id === connectorId);
    this.#renderAgentAccessForConnector(connectorId, match);
  }

  /**
   * Render the access toggle + tool rules for a single connector within the
   * detail modal. If `match` is null the connector is not in the agent's list
   * but we still show the enable toggle (off).
   */
  #renderAgentAccessForConnector(connectorId, match) {
    const body = this.querySelector('#detail-agent-access-body');
    const enabled = match ? !!match.enabled : false;
    body.innerHTML = `
      <div class="agent-access-table" style="margin-top:0.5rem">
        <div style="display:flex;align-items:center;gap:0.75rem;margin-bottom:0.75rem">
          <label class="switch">
            <input type="checkbox" class="access-toggle" ${enabled ? 'checked' : ''} />
            <span class="slider"></span>
          </label>
          <span>${enabled ? 'Enabled' : 'Disabled'}</span>
        </div>
        ${enabled ? `
          <button class="btn-ghost act-tools" type="button">${icons.chevronDown('', 14)} Tool rules</button>
          <div class="tools-editor" data-id="${this.#esc(connectorId)}" hidden></div>
        ` : ''}
      </div>`;

    const toggle = body.querySelector('.access-toggle');
    toggle.addEventListener('change', async () => {
      try {
        await window.setAgentMcpConnectorAccess(this.#selectedAgentId, connectorId, toggle.checked);
        // Re-load to refresh the tool rules section.
        this.#loadAgentAccessForConnector(connectorId);
      } catch (e) {
        toggle.checked = !toggle.checked;
        alert(`Failed to update access: ${e.message}`);
      }
    });

    const toolsBtn = body.querySelector('.act-tools');
    if (toolsBtn) {
      toolsBtn.addEventListener('click', () => {
        const editor = body.querySelector(`.tools-editor[data-id="${CSS.escape(connectorId)}"]`);
        editor.hidden = !editor.hidden;
        if (!editor.hidden) this.#renderToolsEditor(connectorId);
      });
    }
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
