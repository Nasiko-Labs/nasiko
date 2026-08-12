import { icons } from '/common/utils/icons.js';
import { fetchApi, apiFetch } from '/common/services/api.js';
import { authService } from '/common/services/auth-service.js';
import { showToast } from '/common/utils/toast.js';
import { ansiToHtml } from '/common/utils/ansi.js';
import { attachSlidingIndicator } from '/common/utils/tab-indicator.js';
import styles from './agent-card-page.css' with { type: 'css' };
import '/common/components/app-empty-state.js';
import '/common/components/app-modal.js';
import '/common/components/agent-llm-config.js';
import '/common/components/secrets-manager.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

// Tabs marked `managed` render only for callers who can manage the agent
// (owner or superuser — `can_manage` from GET /api/agents/{id}).
const TABS = [
  { key: 'overview', label: 'Overview' },
  { key: 'access', label: 'Access & security', managed: true },
  { key: 'versions', label: 'Versions', managed: true },
  { key: 'configure', label: 'Configure', managed: true },
  { key: 'settings', label: 'Settings', managed: true },
  { key: 'logs', label: 'Logs' },
];

const GRANT_TYPES = [
  { key: 'user', label: 'User', eeOnly: false },
  { key: 'team', label: 'Team', eeOnly: true },
  { key: 'department', label: 'Department', eeOnly: true },
  { key: 'agent', label: 'Agent', eeOnly: false },
];

class AgentCardPage extends HTMLElement {
  #initialized = false;
  #agent = null;
  #agentId = null;
  #canManage = false;
  #logsLoaded = false;
  #secretsLoaded = false;
  #accessLoaded = false;
  #configureLoaded = false;
  #versionsLoaded = false;
  #versions = [];
  // Version targeted by the open rollback modal.
  #rollbackTarget = null;
  #logsTail = 100;
  #logsFollowing = true;
  // Access & security state
  #access = null;
  #granteeTab = 'users';
  #accessFilter = '';
  #grantType = 'user';
  #grantPicked = null;
  #transferPicked = null;
  // Configure state
  #connectors = [];
  #connectorTools = new Map();
  #openConnectors = new Set();

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#agentId = new URLSearchParams(location.search).get('id');
    if (!this.#agentId) {
      this.innerHTML = `
        <div style="display:flex;flex-direction:column;align-items:center;justify-content:center;gap:var(--space-md);min-height:60vh;text-align:center;">
          <app-empty-state
            title="No agent selected"
            description="Open an agent from the hub to see its card, settings, and logs.">
          </app-empty-state>
          <a href="/agents.html" style="color:var(--fg-brand);font-size:var(--font-size-sm);font-weight:600;">Browse the agent hub</a>
        </div>`;
      return;
    }
    this.addEventListener('click', (e) => this.#onActionClick(e));
    this.innerHTML = '<app-skeleton height="400px" style="max-width:900px;margin:0 auto;"></app-skeleton>';
    this.#load();
  }

  async #load() {
    try {
      // GET /api/agents/{id} → SingleResponse envelope {data, status_code, message}
      const resp = await fetchApi(`/agents/${this.#agentId}`);
      this.#agent = resp?.data ?? resp;
    } catch {
      this.#agent = null;
    }
    if (!this.#agent?.name && !this.#agent?.display_name) {
      this.innerHTML = '<p style="color:var(--color-error);">Agent not found.</p>';
      return;
    }
    this.#canManage = await this.#resolveCanManage(this.#agent);
    this.#logsLoaded = false;
    this.#secretsLoaded = false;
    this.#accessLoaded = false;
    this.#configureLoaded = false;
    document.title = `Nasiko — ${this.#agent.display_name || this.#agent.name}`;
    this.#render();
  }

  // `can_manage` is computed server-side with the same predicate the mutating
  // routes enforce. Older servers don't send it — fall back to comparing the
  // caller with owner_id (superusers manage everything).
  async #resolveCanManage(agent) {
    if (typeof agent.can_manage === 'boolean') return agent.can_manage;
    const user = await authService.fetchCurrentUser().catch(() => null);
    if (!user) return false;
    return user.is_superuser === true || (!!agent.owner_id && user.id === agent.owner_id);
  }

  /* ── Page skeleton ─────────────────────────────────────────────────────── */

  #render() {
    const a = this.#agent;
    const displayName = a.display_name || a.name;

    this.innerHTML = `
      <div class="acp-page">
        ${this.#topbarHtml(a)}
        ${this.#heroHtml(a, displayName)}
        <nav class="acp-tabs">
          ${this.#visibleTabs().map((t, i) => `<button class="acp-tab${i === 0 ? ' is-active' : ''}" data-tab="${t.key}">${t.label}</button>`).join('')}
        </nav>
        ${this.#overviewPanelHtml(a)}
        ${this.#canManage ? this.#accessPanelHtml() : ''}
        ${this.#canManage ? this.#versionsPanelHtml() : ''}
        ${this.#canManage ? this.#configurePanelHtml(a) : ''}
        ${this.#canManage ? this.#settingsPanelHtml(a) : ''}
        ${this.#logsPanelHtml()}
      </div>
      ${this.#canManage ? this.#modalsHtml() : ''}
    `;

    this.#wireTabs();
    this.#wireOverview(a);
    this.#wireLogsControls();
    if (this.#canManage) {
      this.#wireSettings();
      this.#wireGrantModal();
      this.#wireTransferModal();
      this.#wireVersionModals();
    }
    this.#loadStats();
    this.#loadResourceUsage();
  }

  #visibleTabs() {
    return TABS.filter((t) => !t.managed || this.#canManage);
  }

  #topbarHtml(a) {
    const actions = this.#canManage && a.status === 'running' ? `
            <button class="acp-action-btn" data-action="restart" title="Restart agent">${icons.refresh('', 14)} Restart</button>
            <button class="acp-action-btn" data-action="stop" title="Stop agent">${icons.square('', 14)} Stop</button>` : '';
    return `
        <div class="acp-topbar">
          <a class="acp-back" href="/your-agents.html" title="Back to Your Agents" aria-label="Back to Your Agents">
            ${icons.x('', 16)}
          </a>
          <div class="acp-topbar-actions">${actions}</div>
        </div>`;
  }

  #heroHtml(a, displayName) {
    const tagsHtml = (a.tags || []).slice(0, 3).map(t =>
      `<span class="acp-tag">${this.#esc(t)}</span>`
    ).join('');
    const extraTagCount = (a.tags || []).length - 3;
    const moreTag = extraTagCount > 0 ? `<span class="acp-tag acp-tag--more">+${extraTagCount}</span>` : '';
    const statusVariant = a.status === 'running' ? 'success' : (a.status === 'error' || a.status === 'failed') ? 'error' : 'neutral';
    const statusLabel = a.status === 'running' ? 'Active' : (a.status || 'Unknown').replace(/^./, c => c.toUpperCase());
    return `
        <div class="acp-title-row">
          <h1 class="acp-name">${this.#esc(displayName)}</h1>
          <span class="acp-version">v${this.#esc(a.version || '?')}</span>
          <span class="acp-verified" title="Registered agent">${icons.checkCircle('', 16)}</span>
          <a class="acp-start-btn" href="/chat.html?agent_id=${encodeURIComponent(a.id)}&agent_name=${encodeURIComponent(displayName)}">
            Start session ${icons.send('', 15)}
          </a>
        </div>

        <div class="acp-badge-row">
          <span class="badge badge--${statusVariant}">${this.#esc(statusLabel)}</span>
          ${a.provider ? `<span class="acp-tag">Author: ${this.#esc(a.provider)}</span>` : ''}
          ${tagsHtml}${moreTag}
        </div>`;
  }

  /* ── Overview tab ──────────────────────────────────────────────────────── */

  #overviewPanelHtml(a) {
    const skills = a.skills || [];
    const skillsHtml = skills.map(s => {
      const href = s.sample_query
        ? `/chat.html?agent_id=${encodeURIComponent(a.id)}&query=${encodeURIComponent(s.sample_query)}`
        : null;
      const wrapper = href ? 'a' : 'div';
      const hrefAttr = href ? ` href="${this.#escAttr(href)}"` : '';
      return `
      <${wrapper} class="acp-skill-card"${hrefAttr}>
        <div class="acp-skill-name">${this.#esc(s.name)}</div>
        <div class="acp-skill-desc">${this.#esc(s.description || '')}</div>
        ${s.sample_query ? `
        <div class="acp-skill-sample">
          <span class="acp-skill-sample-icon">${icons.send('', 14)}</span>
          <span class="acp-skill-sample-text">${this.#esc(s.sample_query)}</span>
        </div>` : ''}
      </${wrapper}>`;
    }).join('');

    const caps = a.capabilities || {};

    return `
        <div class="acp-panel is-active" data-panel="overview">
          <div class="acp-overview-head">
            <p class="acp-description">${this.#esc(a.description || '')}</p>
            <label class="acp-json-toggle">
              <input type="checkbox" id="acp-json-switch" />
              <span class="acp-json-track" aria-hidden="true"></span>
              <span class="acp-json-label">Agent JSON</span>
              ${icons.code('', 15)}
            </label>
          </div>

          <div id="acp-json-view" hidden>
            <div class="acp-json-block">
              <button type="button" class="acp-json-copy" title="Copy JSON" aria-label="Copy JSON">${icons.copy('', 15)}</button>
              <pre><code>${this.#esc(JSON.stringify(a, null, 2))}</code></pre>
            </div>
          </div>

          <div id="acp-overview-body">
          ${skills.length ? `
          <section class="acp-section">
            <h2 class="acp-section-title">Skills</h2>
            <p class="acp-section-sub">What this agent can do. Click a skill to start a session with a sample query.</p>
            <div class="acp-skills-grid">${skillsHtml}</div>
          </section>` : ''}

          <section class="acp-section">
            <h2 class="acp-section-title">Quick performance</h2>
            <p class="acp-section-sub">Recent runtime metrics for this agent.</p>
            <div class="acp-stats-grid" id="acp-stats">
              <div class="acp-stat"><app-skeleton height="48px"></app-skeleton></div>
              <div class="acp-stat"><app-skeleton height="48px"></app-skeleton></div>
              <div class="acp-stat"><app-skeleton height="48px"></app-skeleton></div>
              <div class="acp-stat"><app-skeleton height="48px"></app-skeleton></div>
            </div>
          </section>

          <section class="acp-section">
            <h2 class="acp-section-title">Resource usage</h2>
            <p class="acp-section-sub">What this agent's container is consuming right now.</p>
            <div class="acp-stats-grid" id="acp-resources">
              <div class="acp-stat"><app-skeleton height="48px"></app-skeleton></div>
              <div class="acp-stat"><app-skeleton height="48px"></app-skeleton></div>
              <div class="acp-stat"><app-skeleton height="48px"></app-skeleton></div>
            </div>
          </section>

          <div class="acp-details-row">
            <section class="acp-section acp-details-card">
              <h2 class="acp-section-title">Agent details</h2>
              <dl class="acp-dl">
                <div><dt>Provider</dt><dd>${this.#esc(a.provider || '—')}</dd></div>
                <div><dt>Project URL</dt><dd>${a.project_url ? `<a href="${this.#escAttr(a.project_url)}" target="_blank">${this.#esc(a.project_url)}</a>` : '—'}</dd></div>
                <div><dt>Docs</dt><dd>${a.docs_url ? `<a href="${this.#escAttr(a.docs_url)}" target="_blank">${this.#esc(a.docs_url)}</a>` : '—'}</dd></div>
                <div><dt>ID</dt><dd><code>${this.#esc(a.id)}</code></dd></div>
                <div><dt>Version</dt><dd>${this.#esc(a.version || '—')}</dd></div>
                <div><dt>Protocol</dt><dd>${this.#esc(a.protocol_version || '—')}</dd></div>
                <div><dt>Transport</dt><dd>${this.#esc(a.transport || 'JSONRPC')}</dd></div>
                <div><dt>Default I/O</dt><dd>${this.#esc(a.default_io || 'application/json, text/plain')}</dd></div>
              </dl>
            </section>

            <section class="acp-section acp-details-card">
              <h2 class="acp-section-title">Capabilities</h2>
              <dl class="acp-dl">
                <div><dt>Streaming</dt><dd>${caps.streaming ? 'Yes' : 'No'}</dd></div>
                <div><dt>Push notification</dt><dd>${caps.push_notifications ? 'Supported' : 'Not supported'}</dd></div>
                <div><dt>State history</dt><dd>${caps.state_transition_history ? 'Yes' : 'No'}</dd></div>
                <div><dt>Chat agent</dt><dd>${a.status === 'running' ? 'Enabled' : 'Disabled'}</dd></div>
              </dl>
            </section>
          </div>
          </div>
        </div>`;
  }

  #wireOverview(a) {
    const jsonSwitch = this.querySelector('#acp-json-switch');
    jsonSwitch.addEventListener('change', () => {
      this.querySelector('#acp-json-view').hidden = !jsonSwitch.checked;
      this.querySelector('#acp-overview-body').hidden = jsonSwitch.checked;
    });
    this.querySelector('.acp-json-copy').addEventListener('click', () => {
      navigator.clipboard.writeText(JSON.stringify(a, null, 2))
        .then(() => showToast('Agent JSON copied'))
        .catch(() => showToast('Copy failed'));
    });
  }

  /* ── Tab switching ─────────────────────────────────────────────────────── */

  #wireTabs() {
    attachSlidingIndicator(this.querySelector('.acp-tabs'), '.acp-tab', '.is-active');
    this.querySelector('.acp-tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.acp-tab');
      if (!tab) return;
      this.querySelectorAll('.acp-tab').forEach(t => t.classList.remove('is-active'));
      this.querySelectorAll('.acp-panel').forEach(p => p.classList.remove('is-active'));
      tab.classList.add('is-active');
      this.querySelector(`[data-panel="${tab.dataset.tab}"]`).classList.add('is-active');
      if (tab.dataset.tab === 'logs' && !this.#logsLoaded) this.#loadLogs();
      if (tab.dataset.tab === 'settings' && !this.#secretsLoaded) {
        this.#secretsLoaded = true;
        this.querySelector('#acp-secrets')?.refresh();
      }
      if (tab.dataset.tab === 'access' && !this.#accessLoaded) this.#loadAccess();
      if (tab.dataset.tab === 'versions' && !this.#versionsLoaded) this.#loadVersions();
      if (tab.dataset.tab === 'configure' && !this.#configureLoaded) this.#loadConfigure();
    });
  }

  /* ── Topbar / danger-zone actions (wired once, delegated) ──────────────── */

  async #onActionClick(e) {
    const btn = e.target.closest('[data-action]');
    if (!btn || !this.#agent) return;
    const action = btn.dataset.action;
    if (action === 'restart') this.#runContainerAction(btn, 'restart', 'Restarting...');
    else if (action === 'stop') this.#runContainerAction(btn, 'stop', 'Stopping...');
    else if (action === 'delete') this.#deleteAgent(btn);
    else if (action === 'reupload') this.#openReuploadModal();
    else if (action === 'rollback') this.#openRollbackModal(btn.dataset.version);
  }

  async #runContainerAction(btn, verb, busyLabel) {
    const original = btn.innerHTML;
    btn.disabled = true;
    btn.textContent = busyLabel;
    try {
      const res = await apiFetch(`/containers/${encodeURIComponent(this.#agent.name)}/${verb}`, { method: 'POST' });
      if (!res.ok) throw new Error(await res.text());
      if (verb === 'restart') showToast('Agent restarted');
      location.reload();
    } catch (err) {
      showToast(`Failed to ${verb}: ${err.message}`);
      btn.disabled = false;
      btn.innerHTML = original;
    }
  }

  async #deleteAgent(btn) {
    const displayName = this.#agent.display_name || this.#agent.name;
    if (!confirm(`Delete "${displayName}"? This removes the agent from the registry, revokes all grants, and stops its container.`)) return;
    const original = btn.innerHTML;
    btn.disabled = true;
    btn.textContent = 'Deleting...';
    try {
      const res = await apiFetch(`/agents/${encodeURIComponent(this.#agent.id)}`, { method: 'DELETE' });
      if (!res.ok) throw new Error(await res.text());
      location.href = '/your-agents.html';
    } catch (err) {
      showToast(`Failed to delete: ${err.message}`);
      btn.disabled = false;
      btn.innerHTML = original;
    }
  }

  /* ── Access & security tab ─────────────────────────────────────────────── */

  #accessPanelHtml() {
    return `
        <div class="acp-panel" data-panel="access">
          <div id="acp-access-body"><app-skeleton height="240px"></app-skeleton></div>
        </div>`;
  }

  /* ── Versions tab ──────────────────────────────────────────────────────── */

  #versionsPanelHtml() {
    return `
        <div class="acp-panel" data-panel="versions">
          <section class="acp-section">
            <div class="acp-versions-head">
              <div>
                <h2 class="acp-section-title">Version history</h2>
                <p class="acp-section-sub">Every build of this agent. Re-upload to ship a new
                  version, or roll back to a previous image.</p>
              </div>
              <button type="button" class="acp-primary-btn" data-action="reupload">${icons.upload('', 14)} Re-upload</button>
            </div>
            <div id="acp-versions-body"><app-skeleton height="200px"></app-skeleton></div>
          </section>
        </div>`;
  }

  async #loadVersions() {
    this.#versionsLoaded = true;
    const el = this.querySelector('#acp-versions-body');
    if (!el) return;
    try {
      const resp = await fetchApi(`/agents/${this.#agent.id}/versions`);
      this.#versions = (resp?.data ?? resp) || [];
    } catch (e) {
      el.innerHTML = `<div class="acp-stats-empty"><app-empty-state
        title="Version history unavailable"
        description="${this.#escAttr(e.message)}"
        icon="${this.#escAttr(icons.layers('', 32))}"></app-empty-state></div>`;
      return;
    }
    this.#renderVersions();
  }

  #renderVersions() {
    const el = this.querySelector('#acp-versions-body');
    if (!el) return;

    if (!this.#versions.length) {
      el.innerHTML = `<div class="acp-stats-empty"><app-empty-state
        title="No versions recorded"
        description="Versions appear here once this agent has been built through upload, push or re-upload."
        icon="${this.#escAttr(icons.layers('', 32))}"></app-empty-state></div>`;
      return;
    }

    const statusBadge = (v) => {
      if (v.is_active) return '<span class="badge badge--brand"><span class="badge__dot"></span>Active</span>';
      if (v.status === 'archived') return '<span class="badge badge--muted">Archived</span>';
      return `<span class="badge badge--muted">${this.#esc(v.status || '—')}</span>`;
    };

    el.innerHTML = `
      <table class="acp-table">
        <thead>
          <tr>
            <th>Version</th><th>Status</th><th>Image</th><th>Changelog</th><th>Built</th><th></th>
          </tr>
        </thead>
        <tbody>
          ${this.#versions.map((v) => `
            <tr>
              <td><code>${this.#esc(v.version)}</code></td>
              <td>${statusBadge(v)}</td>
              <td class="acp-td-muted"><code>${this.#esc(v.image_tag || '—')}</code></td>
              <td class="acp-td-muted">${this.#esc(v.changelog || '—')}</td>
              <td class="acp-td-muted">${v.created_at ? new Date(v.created_at).toLocaleString() : '—'}</td>
              <td>
                ${v.is_active || !v.can_rollback ? '' : `
                  <button type="button" class="acp-action-btn" data-action="rollback"
                    data-version="${this.#escAttr(v.version)}">Roll back</button>`}
              </td>
            </tr>`).join('')}
        </tbody>
      </table>`;
  }

  #wireVersionModals() {
    const reupload = this.querySelector('#acp-reupload-modal');
    this.querySelector('#acp-reupload-cancel').addEventListener('click', () => reupload.close());
    this.querySelector('#acp-reupload-submit').addEventListener('click', () => this.#submitReupload());

    const rollback = this.querySelector('#acp-rollback-modal');
    this.querySelector('#acp-rollback-cancel').addEventListener('click', () => rollback.close());
    this.querySelector('#acp-rollback-submit').addEventListener('click', () => this.#submitRollback());
  }

  #openReuploadModal() {
    this.querySelector('#acp-reupload-file').value = '';
    this.querySelector('#acp-reupload-version').value = 'patch';
    this.querySelector('#acp-reupload-changelog').value = '';
    this.querySelector('#acp-reupload-error').hidden = true;
    this.querySelector('#acp-reupload-modal').open();
  }

  /// `PUT /api/agents/{id}/update` — multipart `source` (.zip) plus an optional
  /// `version` (semver or one of auto|patch|minor|major) and `changelog`.
  /// Queues a build; progress shows up under Builds.
  async #submitReupload() {
    const submit = this.querySelector('#acp-reupload-submit');
    const error = this.querySelector('#acp-reupload-error');
    const file = this.querySelector('#acp-reupload-file').files[0];
    const version = this.querySelector('#acp-reupload-version').value.trim();
    const changelog = this.querySelector('#acp-reupload-changelog').value.trim();

    error.hidden = true;
    if (!file) {
      this.#showModalError(error, 'Choose a .zip archive to upload.');
      return;
    }
    if (!version) {
      this.#showModalError(error, 'A version or bump strategy is required.');
      return;
    }

    const form = new FormData();
    form.append('source', file, file.name);
    form.append('version', version);
    if (changelog) form.append('changelog', changelog);

    submit.disabled = true;
    try {
      const res = await apiFetch(`/agents/${encodeURIComponent(this.#agent.id)}/update`, {
        method: 'PUT',
        body: form,
      });
      if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
      const body = await res.json().catch(() => null);
      this.querySelector('#acp-reupload-modal').close();
      showToast(`Build queued for v${body?.new_version || version}`);
      this.#refreshVersions();
    } catch (err) {
      this.#showModalError(error, err.message);
    } finally {
      submit.disabled = false;
    }
  }

  #openRollbackModal(targetVersion) {
    this.#rollbackTarget = targetVersion;
    this.querySelector('#acp-rollback-summary').textContent =
      `Redeploys the image built for ${targetVersion} and archives the current version. `
      + 'The rollback runs as a build, so it appears in the version history too.';
    this.querySelector('#acp-rollback-reason').value = '';
    this.querySelector('#acp-rollback-error').hidden = true;
    this.querySelector('#acp-rollback-modal').open();
  }

  async #submitRollback() {
    const submit = this.querySelector('#acp-rollback-submit');
    const error = this.querySelector('#acp-rollback-error');
    const reason = this.querySelector('#acp-rollback-reason').value.trim();
    error.hidden = true;

    submit.disabled = true;
    try {
      const res = await apiFetch(`/agents/${encodeURIComponent(this.#agent.id)}/rollback`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ target_version: this.#rollbackTarget, reason: reason || null }),
      });
      if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
      const body = await res.json().catch(() => null);
      this.querySelector('#acp-rollback-modal').close();
      showToast(`Rollback to ${body?.rolled_back_to || this.#rollbackTarget} queued`);
      this.#refreshVersions();
    } catch (err) {
      this.#showModalError(error, err.message);
    } finally {
      submit.disabled = false;
    }
  }

  /// Builds are asynchronous, so the row that just changed won't be `active`
  /// yet — re-reading is still the honest thing to show.
  #refreshVersions() {
    this.#versionsLoaded = false;
    this.#loadVersions();
  }

  #showModalError(el, message) {
    el.textContent = message;
    el.hidden = false;
  }

  // Feature-detects what the running edition serves: EE answers
  // /teams and /departments with arrays; OSS has no such routes (the request
  // falls through to the agent proxy and fails), so those grantee tabs hide.
  async #loadAccess() {
    this.#accessLoaded = true;
    const id = this.#agent.id;
    const [visibility, users, agentGrants, teams, departments] = await Promise.all([
      this.#fetchVisibility(id),
      this.#fetchArray(`/agents/${id}/users`),
      this.#fetchArray(`/agents/${id}/grants/agents`),
      this.#fetchArray(`/agents/${id}/teams`),
      this.#fetchArray(`/agents/${id}/departments`),
    ]);
    this.#access = {
      visibility,
      users: (users || []).map((u) => ({
        id: u.id ?? u.user_id ?? '',
        name: u.username || u.user_id || u.id || '',
        email: u.email || '',
        role: u.role || '',
      })),
      agents: this.#normalizeAgentGrants(agentGrants, visibility),
      teams,
      departments,
    };
    if (this.#granteeTab !== 'users' && this.#granteeTab !== 'agents'
        && !Array.isArray(this.#access[this.#granteeTab])) {
      this.#granteeTab = 'users';
    }
    this.#renderAccess();
  }

  // `null` means "this edition doesn't serve the route" — the caller uses that
  // to decide whether Team/Department exist at all, so an enveloped array must
  // NOT be mistaken for a missing route. EE answers /users with a bare array
  // but /teams and /departments as {data:{accessible_teams|accessible_departments}},
  // and reading only the bare form is what silently hid both grantee types.
  async #fetchArray(path) {
    try {
      const v = await fetchApi(path);
      if (Array.isArray(v)) return v;
      const inner = v?.data;
      if (Array.isArray(inner)) return inner;
      const nested = Object.values(inner || {}).find(Array.isArray);
      return nested ?? null;
    } catch {
      return null;
    }
  }

  async #fetchVisibility(id) {
    try {
      const v = await fetchApi(`/agents/${id}/visibility`);
      return typeof v?.is_public === 'boolean' ? v : null;
    } catch {
      return null;
    }
  }

  // OSS answers GET /grants/agents with [{target_agent_id, target_name}]; EE
  // doesn't serve that read — derive its agent grants from visibility.grants.
  #normalizeAgentGrants(rows, visibility) {
    if (Array.isArray(rows)) {
      return rows.map((r) => ({ id: r.target_agent_id, name: r.target_name || '' }));
    }
    const grants = visibility?.grants;
    if (!Array.isArray(grants)) return [];
    return grants
      .filter((g) => g.grant_type === 'agent')
      .map((g) => ({ id: g.grantee_id, name: '' }));
  }

  // Direct user shares: EE lists ALL users with access (owner + inherited), so
  // "direct" comes from the grant rows; OSS's /users listing is direct-only.
  #directUserIds() {
    const grants = this.#access.visibility?.grants;
    if (Array.isArray(grants)) {
      return new Set(grants.filter((g) => g.grant_type === 'user').map((g) => g.grantee_id));
    }
    return new Set(this.#access.users.map((u) => u.id));
  }

  #granteeTabDefs() {
    const defs = [];
    if (Array.isArray(this.#access.departments)) defs.push({ key: 'departments', label: 'Departments' });
    if (Array.isArray(this.#access.teams)) defs.push({ key: 'teams', label: 'Teams' });
    defs.push({ key: 'users', label: 'Users' });
    defs.push({ key: 'agents', label: 'Agents' });
    return defs;
  }

  #renderAccess() {
    const body = this.querySelector('#acp-access-body');
    if (!body) return;
    const acc = this.#access;
    const isEe = Array.isArray(acc.teams) || Array.isArray(acc.departments);
    const privateSub = isEe
      ? 'Only granted departments, teams and users'
      : 'Only granted users and agents';

    const visibilitySection = acc.visibility === null ? '' : `
      <section class="acp-section">
        <h2 class="acp-section-title">Visibility</h2>
        <p class="acp-section-sub">Who can discover and use this agent?</p>
        <div class="acp-vis-options" role="radiogroup" aria-label="Agent visibility">
          ${this.#visOptionHtml('public', 'Public', 'Anyone in your organisation can discover and use it', acc.visibility.is_public)}
          ${this.#visOptionHtml('private', 'Private', privateSub, !acc.visibility.is_public)}
        </div>
      </section>`;

    body.innerHTML = `
      ${visibilitySection}
      <section class="acp-section">
        <div class="acp-access-head">
          <div>
            <h2 class="acp-section-title">Access</h2>
            <p class="acp-section-sub">Grant access to ${isEe ? 'users, teams, or departments' : 'users or agents'}. ${isEe ? 'Access is automatically inherited by members.' : ''}</p>
          </div>
          <button type="button" class="acp-primary-btn" id="acp-grant-open">${icons.plus('', 14)} Grant access</button>
        </div>
        <div class="acp-access-search">
          ${icons.search('acp-access-search-icon', 14)}
          <input type="text" id="acp-access-filter" placeholder="Search ${this.#granteeTabDefs().map(d => d.label.toLowerCase()).join(', ')}"
            value="${this.#escAttr(this.#accessFilter)}" autocomplete="off" />
        </div>
        <div class="acp-subtabs" id="acp-grantee-tabs">
          ${this.#granteeTabDefs().map(d => `<button type="button" class="acp-subtab${d.key === this.#granteeTab ? ' is-active' : ''}" data-grantee-tab="${d.key}">${d.label}</button>`).join('')}
        </div>
        <div id="acp-access-table">${this.#accessTableHtml()}</div>
      </section>`;

    this.#wireAccess(body);
  }

  #visOptionHtml(value, label, sub, checked) {
    return `
      <label class="acp-vis-option">
        <input type="radio" name="acp-visibility" value="${value}" ${checked ? 'checked' : ''} />
        <span class="acp-vis-radio" aria-hidden="true"></span>
        <span class="acp-vis-text">
          <span class="acp-vis-label">${label}</span>
          <span class="acp-vis-sub">${sub}</span>
        </span>
      </label>`;
  }

  #wireAccess(body) {
    body.querySelectorAll('input[name="acp-visibility"]').forEach((radio) => {
      radio.addEventListener('change', () => this.#setVisibility(radio.value === 'public'));
    });
    const filter = body.querySelector('#acp-access-filter');
    filter?.addEventListener('input', () => {
      this.#accessFilter = filter.value;
      this.querySelector('#acp-access-table').innerHTML = this.#accessTableHtml();
      this.#wireAccessTable();
    });
    const granteeTabs = body.querySelector('#acp-grantee-tabs');
    if (granteeTabs) attachSlidingIndicator(granteeTabs, '.acp-subtab', '.is-active', { pill: true });
    granteeTabs?.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-grantee-tab]');
      if (!btn) return;
      this.#granteeTab = btn.dataset.granteeTab;
      body.querySelectorAll('.acp-subtab').forEach(t =>
        t.classList.toggle('is-active', t.dataset.granteeTab === this.#granteeTab));
      this.querySelector('#acp-access-table').innerHTML = this.#accessTableHtml();
      this.#wireAccessTable();
    });
    body.querySelector('#acp-grant-open')?.addEventListener('click', () => this.#openGrantModal());
    this.#wireAccessTable();
  }

  async #setVisibility(isPublic) {
    try {
      const res = await apiFetch(`/agents/${encodeURIComponent(this.#agent.id)}/grants/public`, {
        method: isPublic ? 'POST' : 'DELETE',
      });
      if (!res.ok && res.status !== 404) throw new Error(await res.text());
      this.#access.visibility.is_public = isPublic;
      showToast(isPublic ? 'Agent is now public' : 'Agent is now private');
    } catch (err) {
      showToast(`Failed to change visibility: ${err.message}`);
      this.#renderAccess();
    }
  }

  #matchesFilter(...fields) {
    const q = this.#accessFilter.trim().toLowerCase();
    if (!q) return true;
    return fields.some((f) => (f || '').toLowerCase().includes(q));
  }

  #accessTableHtml() {
    switch (this.#granteeTab) {
      case 'departments': return this.#departmentsTableHtml();
      case 'teams': return this.#teamsTableHtml();
      case 'agents': return this.#agentsTableHtml();
      default: return this.#usersTableHtml();
    }
  }

  #accessEmptyHtml() {
    const label = this.#granteeTabDefs().find(d => d.key === this.#granteeTab)?.label || 'entries';
    return `
      <div class="acp-access-empty">
        <span class="acp-access-empty-title">No ${label.toLowerCase()} have access yet</span>
        <span class="acp-access-empty-sub">Use Grant access to share this agent.</span>
      </div>`;
  }

  #usersTableHtml() {
    const ownerId = this.#agent.owner_id;
    const direct = this.#directUserIds();
    let rows = this.#access.users.filter((u) => this.#matchesFilter(u.name, u.email));
    // OSS's grant listing omits the owner — surface them anyway.
    if (ownerId && !this.#access.users.some((u) => u.id === ownerId)) {
      const self = authService.getCurrentUserId() === ownerId ? authService.getCurrentUser() : null;
      rows = [{ id: ownerId, name: self || this.#shortId(ownerId), email: '', role: '' }, ...rows];
    }
    if (!rows.length) return this.#accessEmptyHtml();
    return `
      <table class="acp-table">
        <thead><tr><th>User</th><th>Email</th><th>Role</th><th>Grant</th><th class="acp-th-actions"></th></tr></thead>
        <tbody>
          ${rows.map((u) => {
            const isOwner = u.id === ownerId;
            const isDirect = direct.has(u.id);
            const grant = isOwner
              ? '<span class="acp-grant-badge is-owner">Owner</span>'
              : isDirect
                ? '<span class="acp-grant-badge is-direct">Direct</span>'
                : '<span class="acp-grant-badge">Inherited</span>';
            const action = isOwner
              ? `<button type="button" class="acp-link-btn" data-transfer-open>Transfer ownership</button>`
              : isDirect
                ? this.#revokeBtnHtml('user', u.id, u.name)
                : '';
            return `
            <tr>
              <td class="acp-td-name">${this.#esc(u.name)}</td>
              <td class="acp-td-muted">${this.#esc(u.email || '—')}</td>
              <td class="acp-td-muted">${this.#esc(u.role || '—')}</td>
              <td>${grant}</td>
              <td class="acp-td-actions">${action}</td>
            </tr>`;
          }).join('')}
        </tbody>
      </table>`;
  }

  #teamsTableHtml() {
    const rows = (this.#access.teams || []).filter((t) => this.#matchesFilter(t.name));
    if (!rows.length) return this.#accessEmptyHtml();
    return `
      <table class="acp-table">
        <thead><tr><th>Team</th><th>Members</th><th>Grant</th><th class="acp-th-actions"></th></tr></thead>
        <tbody>
          ${rows.map((t) => `
            <tr>
              <td class="acp-td-name">${this.#esc(t.name)}</td>
              <td class="acp-td-muted">${t.members_count ?? '—'}</td>
              <td><span class="acp-grant-badge is-direct">Direct</span></td>
              <td class="acp-td-actions">${this.#revokeBtnHtml('team', t.id, t.name)}</td>
            </tr>`).join('')}
        </tbody>
      </table>`;
  }

  #departmentsTableHtml() {
    const rows = (this.#access.departments || []).filter((d) => this.#matchesFilter(d.name));
    if (!rows.length) return this.#accessEmptyHtml();
    return `
      <table class="acp-table">
        <thead><tr><th>Department</th><th>Members</th><th>Teams</th><th class="acp-th-actions"></th></tr></thead>
        <tbody>
          ${rows.map((d) => `
            <tr>
              <td class="acp-td-name">${this.#esc(d.name)}</td>
              <td class="acp-td-muted">${d.members_count ?? '—'}</td>
              <td class="acp-td-muted">${d.teams_count ?? '—'}</td>
              <td class="acp-td-actions">${this.#revokeBtnHtml('department', d.id, d.name)}</td>
            </tr>`).join('')}
        </tbody>
      </table>`;
  }

  #agentsTableHtml() {
    const rows = (this.#access.agents || []).filter((g) => this.#matchesFilter(g.name, g.id));
    if (!rows.length) return this.#accessEmptyHtml();
    return `
      <table class="acp-table">
        <thead><tr><th>Agent</th><th>Grant</th><th class="acp-th-actions"></th></tr></thead>
        <tbody>
          ${rows.map((g) => `
            <tr>
              <td class="acp-td-name">${this.#esc(g.name || this.#shortId(g.id))}</td>
              <td><span class="acp-grant-badge is-direct">Direct</span></td>
              <td class="acp-td-actions">${this.#revokeBtnHtml('agent', g.id, g.name || g.id)}</td>
            </tr>`).join('')}
        </tbody>
      </table>`;
  }

  #revokeBtnHtml(kind, id, name) {
    return `<button type="button" class="acp-icon-btn acp-icon-btn--danger" data-revoke-kind="${kind}"
      data-revoke-id="${this.#escAttr(id)}" title="Revoke access for ${this.#escAttr(name)}"
      aria-label="Revoke access for ${this.#escAttr(name)}">${icons.trash('', 14)}</button>`;
  }

  #wireAccessTable() {
    const table = this.querySelector('#acp-access-table');
    if (!table) return;
    table.querySelectorAll('[data-revoke-kind]').forEach((btn) => {
      btn.addEventListener('click', () => this.#revokeGrant(btn.dataset.revokeKind, btn.dataset.revokeId));
    });
    table.querySelector('[data-transfer-open]')?.addEventListener('click', () => this.#openTransferModal());
  }

  async #revokeGrant(kind, granteeId) {
    if (!confirm('Revoke this access grant?')) return;
    const paths = { user: 'users', team: 'teams', department: 'departments', agent: 'agents' };
    try {
      const res = await apiFetch(
        `/agents/${encodeURIComponent(this.#agent.id)}/grants/${paths[kind]}/${encodeURIComponent(granteeId)}`,
        { method: 'DELETE' },
      );
      if (!res.ok && res.status !== 404) throw new Error(await res.text());
      showToast('Access revoked');
    } catch (err) {
      showToast(`Failed to revoke: ${err.message}`);
      return;
    }
    this.#loadAccess();
  }

  /* ── Grant-access modal ────────────────────────────────────────────────── */

  #modalsHtml() {
    return `
      <app-modal id="acp-grant-modal" heading="Grant access">
        <div class="acp-grant-form">
          <div class="acp-grant-types" id="acp-grant-types" role="radiogroup" aria-label="Grant type"></div>
          <div class="acp-picker">
            <input type="text" id="acp-grant-query" placeholder="Search users" autocomplete="off" />
            <div class="acp-picker-results" id="acp-grant-results" hidden></div>
          </div>
          <div class="acp-picker-picked" id="acp-grant-picked" hidden></div>
          <p class="acp-form-error" id="acp-grant-error" hidden></p>
        </div>
        <div data-slot="footer">
          <button type="button" class="acp-action-btn" id="acp-grant-cancel">Cancel</button>
          <button type="button" class="acp-primary-btn" id="acp-grant-submit" disabled>Grant access</button>
        </div>
      </app-modal>
      <app-modal id="acp-transfer-modal" heading="Transfer ownership">
        <div class="acp-grant-form">
          <p class="acp-section-sub">The new owner gains full control of this agent — its grants, secrets, and lifecycle. You keep access only if a grant covers you.</p>
          <div class="acp-picker">
            <input type="text" id="acp-transfer-query" placeholder="Search users" autocomplete="off" />
            <div class="acp-picker-results" id="acp-transfer-results" hidden></div>
          </div>
          <div class="acp-picker-picked" id="acp-transfer-picked" hidden></div>
          <p class="acp-form-error" id="acp-transfer-error" hidden></p>
        </div>
        <div data-slot="footer">
          <button type="button" class="acp-action-btn" id="acp-transfer-cancel">Cancel</button>
          <button type="button" class="acp-primary-btn" id="acp-transfer-submit" disabled>Transfer ownership</button>
        </div>
      </app-modal>
      <app-modal id="acp-reupload-modal" heading="Re-upload agent">
        <div class="acp-grant-form">
          <p class="acp-section-sub">Uploads a new source archive and queues a build. The running
            container is replaced once the build succeeds.</p>
          <label class="acp-field">
            <span class="acp-field-label">Source archive (.zip)</span>
            <input type="file" id="acp-reupload-file" accept=".zip,application/zip" required />
          </label>
          <label class="acp-field">
            <span class="acp-field-label">Version</span>
            <input type="text" id="acp-reupload-version" value="patch" autocomplete="off" />
            <span class="acp-field-hint">A semver string (e.g. 1.2.3), or one of
              <code>auto</code>, <code>patch</code>, <code>minor</code>, <code>major</code>.</span>
          </label>
          <label class="acp-field">
            <span class="acp-field-label">Changelog (optional)</span>
            <input type="text" id="acp-reupload-changelog" autocomplete="off"
              placeholder="What changed in this version?" />
          </label>
          <p class="acp-form-error" id="acp-reupload-error" hidden></p>
        </div>
        <div data-slot="footer">
          <button type="button" class="acp-action-btn" id="acp-reupload-cancel">Cancel</button>
          <button type="button" class="acp-primary-btn" id="acp-reupload-submit">Queue build</button>
        </div>
      </app-modal>
      <app-modal id="acp-rollback-modal" heading="Roll back version">
        <div class="acp-grant-form">
          <p class="acp-section-sub" id="acp-rollback-summary"></p>
          <label class="acp-field">
            <span class="acp-field-label">Reason (optional)</span>
            <input type="text" id="acp-rollback-reason" autocomplete="off"
              placeholder="Recorded against the rollback build" />
          </label>
          <p class="acp-form-error" id="acp-rollback-error" hidden></p>
        </div>
        <div data-slot="footer">
          <button type="button" class="acp-action-btn" id="acp-rollback-cancel">Cancel</button>
          <button type="button" class="acp-primary-btn" id="acp-rollback-submit">Roll back</button>
        </div>
      </app-modal>`;
  }

  #openGrantModal() {
    this.#grantPicked = null;
    this.#grantType = 'user';
    this.#renderGrantTypes();
    this.#setPicked('grant', null);
    const query = this.querySelector('#acp-grant-query');
    query.value = '';
    this.querySelector('#acp-grant-results').hidden = true;
    this.querySelector('#acp-grant-error').hidden = true;
    this.querySelector('#acp-grant-modal').open();
    query.focus();
  }

  #renderGrantTypes() {
    const isEe = Array.isArray(this.#access?.teams) || Array.isArray(this.#access?.departments);
    const el = this.querySelector('#acp-grant-types');
    el.innerHTML = GRANT_TYPES
      .filter((t) => !t.eeOnly || isEe)
      .map((t) => `
        <label class="acp-grant-type${t.key === this.#grantType ? ' is-active' : ''}">
          <input type="radio" name="acp-grant-type" value="${t.key}" ${t.key === this.#grantType ? 'checked' : ''} />
          ${t.label}
        </label>`).join('');
    el.querySelectorAll('input').forEach((radio) => {
      radio.addEventListener('change', () => {
        this.#grantType = radio.value;
        el.querySelectorAll('.acp-grant-type').forEach((l) =>
          l.classList.toggle('is-active', l.querySelector('input').value === radio.value));
        this.#setPicked('grant', null);
        const query = this.querySelector('#acp-grant-query');
        query.value = '';
        query.placeholder = `Search ${this.#grantType}s`;
        this.querySelector('#acp-grant-results').hidden = true;
      });
    });
  }

  #wireGrantModal() {
    const query = this.querySelector('#acp-grant-query');
    query.addEventListener('input', this.#debounce(async () => {
      await this.#searchInto(this.#grantType, query.value, '#acp-grant-results', (picked) => {
        this.#setPicked('grant', picked);
      });
    }, 250));
    this.querySelector('#acp-grant-cancel').addEventListener('click', () =>
      this.querySelector('#acp-grant-modal').close());
    this.querySelector('#acp-grant-submit').addEventListener('click', () => this.#submitGrant());
  }

  #wireTransferModal() {
    const query = this.querySelector('#acp-transfer-query');
    query.addEventListener('input', this.#debounce(async () => {
      await this.#searchInto('user', query.value, '#acp-transfer-results', (picked) => {
        this.#setPicked('transfer', picked);
      });
    }, 250));
    this.querySelector('#acp-transfer-cancel').addEventListener('click', () =>
      this.querySelector('#acp-transfer-modal').close());
    this.querySelector('#acp-transfer-submit').addEventListener('click', () => this.#submitTransfer());
  }

  #openTransferModal() {
    this.#setPicked('transfer', null);
    const query = this.querySelector('#acp-transfer-query');
    query.value = '';
    this.querySelector('#acp-transfer-results').hidden = true;
    this.querySelector('#acp-transfer-error').hidden = true;
    this.querySelector('#acp-transfer-modal').open();
    query.focus();
  }

  #setPicked(which, picked) {
    if (which === 'grant') this.#grantPicked = picked; else this.#transferPicked = picked;
    const chip = this.querySelector(`#acp-${which}-picked`);
    const submit = this.querySelector(`#acp-${which}-submit`);
    if (!chip || !submit) return;
    if (picked) {
      chip.innerHTML = `<span class="acp-chip">${this.#esc(picked.label)}<button type="button" class="acp-chip-x" aria-label="Clear selection">${icons.x('', 12)}</button></span>`;
      chip.hidden = false;
      chip.querySelector('.acp-chip-x').addEventListener('click', () => this.#setPicked(which, null));
    } else {
      chip.innerHTML = '';
      chip.hidden = true;
    }
    submit.disabled = !picked;
  }

  // Searches the picker source for a grant type and renders clickable results.
  async #searchInto(type, rawQuery, resultsSelector, onPick) {
    const results = this.querySelector(resultsSelector);
    const q = rawQuery.trim();
    if (q.length < 2) {
      results.hidden = true;
      return;
    }
    let options = [];
    try {
      options = await this.#searchGrantees(type, q);
    } catch {
      options = [];
    }
    results.innerHTML = options.length
      ? options.map((o, i) => `<button type="button" class="acp-picker-option" data-i="${i}">
          <span class="acp-picker-option-label">${this.#esc(o.label)}</span>
          ${o.sub ? `<span class="acp-picker-option-sub">${this.#esc(o.sub)}</span>` : ''}
        </button>`).join('')
      : '<div class="acp-picker-none">No matches</div>';
    results.hidden = false;
    results.querySelectorAll('.acp-picker-option').forEach((btn) => {
      btn.addEventListener('click', () => {
        onPick(options[Number(btn.dataset.i)]);
        results.hidden = true;
      });
    });
  }

  async #searchGrantees(type, q) {
    const enc = encodeURIComponent(q);
    if (type === 'user') {
      const resp = await fetchApi(`/search/users?q=${enc}`);
      return (resp?.data || []).map((u) => ({ id: u.id, label: u.display_name || u.username, sub: u.email || '' }));
    }
    if (type === 'agent') {
      const resp = await fetchApi(`/search/agents?q=${enc}`);
      return (resp?.agents || [])
        .filter((r) => r.id !== this.#agent.id)
        .map((r) => ({ id: r.id, label: r.display_name || r.name, sub: '' }));
    }
    if (type === 'team') {
      const resp = await fetchApi(`/search/teams?q=${enc}`);
      return (resp?.data || []).map((t) => ({ id: t.id, label: t.name, sub: '' }));
    }
    const resp = await fetchApi(`/search/departments?q=${enc}`);
    return (resp?.data || []).map((d) => ({ id: d.id, label: d.name, sub: d.description || '' }));
  }

  async #submitGrant() {
    if (!this.#grantPicked) return;
    const err = this.querySelector('#acp-grant-error');
    err.hidden = true;
    try {
      await this.#grantEntity(this.#grantType, this.#grantPicked.id);
      this.querySelector('#acp-grant-modal').close();
      showToast(`Access granted to ${this.#grantPicked.label}`);
      this.#granteeTab = { user: 'users', team: 'teams', department: 'departments', agent: 'agents' }[this.#grantType];
      this.#loadAccess();
    } catch (e) {
      err.textContent = `Failed to grant access: ${e.message}`;
      err.hidden = false;
    }
  }

  // EE mounts POST /grants/{kind}/{id}; OSS mounts POST /grants/{kind} with a
  // JSON body. Try the path form first — on OSS it answers 405 (the path only
  // serves DELETE there), never a false success — then fall back.
  async #grantEntity(type, granteeId) {
    const kind = { user: 'users', team: 'teams', department: 'departments', agent: 'agents' }[type];
    const base = `/agents/${encodeURIComponent(this.#agent.id)}/grants/${kind}`;
    const res = await apiFetch(`${base}/${encodeURIComponent(granteeId)}`, { method: 'POST' });
    if (res.ok) return;
    if (res.status !== 404 && res.status !== 405) throw new Error(await res.text());
    const bodyKey = type === 'user' ? 'user_id' : 'agent_id';
    const fallback = await apiFetch(base, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ [bodyKey]: granteeId }),
    });
    if (!fallback.ok) throw new Error(await fallback.text());
  }

  async #submitTransfer() {
    if (!this.#transferPicked) return;
    if (!confirm(`Transfer ownership to ${this.#transferPicked.label}? This cannot be undone from here.`)) return;
    const err = this.querySelector('#acp-transfer-error');
    err.hidden = true;
    try {
      // OSS expects {new_owner_id}, EE expects {owner_id} — send both keys;
      // each edition deserializes its own and ignores the other.
      const res = await apiFetch(`/agents/${encodeURIComponent(this.#agent.id)}/owner`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ owner_id: this.#transferPicked.id, new_owner_id: this.#transferPicked.id }),
      });
      if (!res.ok) throw new Error(await res.text());
      this.querySelector('#acp-transfer-modal').close();
      showToast(`Ownership transferred to ${this.#transferPicked.label}`);
      this.#load();
    } catch (e) {
      err.textContent = `Failed to transfer: ${e.message}`;
      err.hidden = false;
    }
  }

  /* ── Configure tab (MCP servers + LLM router) ──────────────────────────── */

  #configurePanelHtml(a) {
    return `
        <div class="acp-panel" data-panel="configure">
          <section class="acp-section">
            <h2 class="acp-section-title">MCP</h2>
            <p class="acp-section-sub">MCP servers this agent may use. Allow or block each tool individually.</p>
            <div id="acp-mcp-list"><app-skeleton height="160px"></app-skeleton></div>
          </section>
          <section class="acp-section">
            <h2 class="acp-section-title">LLM Router</h2>
            <p class="acp-section-sub">Which models this agent runs on. Overrides here apply to this agent only.</p>
            <agent-llm-config agent-id="${this.#escAttr(a.id)}"></agent-llm-config>
          </section>
        </div>`;
  }

  async #loadConfigure() {
    this.#configureLoaded = true;
    const list = this.querySelector('#acp-mcp-list');
    if (!list) return;
    try {
      const resp = await window.fetchAgentMcpConnectors(this.#agent.id);
      this.#connectors = resp?.data?.connectors || [];
    } catch (e) {
      list.innerHTML = `<p class="acp-section-sub">Failed to load MCP connectors: ${this.#esc(e.message)}</p>`;
      return;
    }
    // Fetch every connector's tools up front so each card can show its
    // "{n} of {m} tools allowed" summary without waiting for an expand.
    await Promise.all(this.#connectors.map(async (c) => {
      try {
        const resp = await window.fetchAgentMcpConnectorTools(this.#agent.id, c.connector_id);
        this.#connectorTools.set(c.connector_id, resp?.data?.tools || []);
      } catch {
        this.#connectorTools.set(c.connector_id, []);
      }
    }));
    this.#renderConnectors();
  }

  #renderConnectors() {
    const list = this.querySelector('#acp-mcp-list');
    if (!list) return;
    if (!this.#connectors.length) {
      list.innerHTML = `
        <div class="acp-access-empty">
          <span class="acp-access-empty-title">No MCP servers available</span>
          <span class="acp-access-empty-sub">Connect servers on the MCP page to make their tools available here.</span>
        </div>`;
      return;
    }
    list.innerHTML = this.#connectors.map((c) => this.#connectorCardHtml(c)).join('');
    this.#wireConnectors(list);
  }

  #connectorCardHtml(c) {
    const name = c.display_name || c.name || 'Connector';
    const tools = this.#connectorTools.get(c.connector_id) || [];
    const allowed = tools.filter((t) => t.stance !== 'deny').length;
    const summary = c.enabled === false
      ? 'Disabled'
      : tools.length ? `${allowed} of ${tools.length} tools allowed` : 'No tools synced yet';
    const open = this.#openConnectors.has(c.connector_id);
    const logo = c.logo_url
      ? `<img class="acp-mcp-logo" src="${this.#escAttr(c.logo_url)}" alt="" />`
      : `<span class="acp-mcp-initial">${this.#esc(name.charAt(0).toUpperCase())}</span>`;
    return `
      <div class="acp-mcp-card${c.enabled === false ? ' is-disabled' : ''}" data-connector="${this.#escAttr(c.connector_id)}">
        <div class="acp-mcp-head">
          <button type="button" class="acp-icon-btn acp-mcp-toggle-open" aria-expanded="${open}"
            aria-label="${open ? 'Collapse' : 'Expand'} ${this.#escAttr(name)}">
            ${open ? icons.chevronUp('', 16) : icons.chevronDown('', 16)}
          </button>
          ${logo}
          <span class="acp-mcp-name">${this.#esc(name)}</span>
          <span class="acp-mcp-summary">${this.#esc(summary)}</span>
          <label class="acp-json-toggle acp-mcp-enable" title="${c.enabled === false ? 'Enable' : 'Disable'} ${this.#escAttr(name)}">
            <input type="checkbox" class="acp-mcp-enable-input" ${c.enabled === false ? '' : 'checked'} />
            <span class="acp-json-track" aria-hidden="true"></span>
          </label>
        </div>
        ${open ? this.#connectorToolsHtml(c, tools) : ''}
      </div>`;
  }

  #connectorToolsHtml(c, tools) {
    const disabled = c.enabled === false;
    const note = disabled
      ? `<div class="acp-mcp-note">${icons.info('', 12)} This server is disabled for this agent — tool rules apply once it is re-enabled.</div>`
      : '';
    if (!tools.length) {
      return `<div class="acp-mcp-tools">${note}<div class="acp-mcp-note">This connector exposes no tools yet.</div></div>`;
    }
    return `
      <div class="acp-mcp-tools">
        ${note}
        ${tools.map((t, i) => `
          <div class="acp-mcp-tool${disabled ? ' is-dim' : ''}">
            <span class="acp-mcp-tool-name">${this.#esc(t.name)}</span>
            <span class="acp-mcp-tool-desc">${this.#esc(t.description || '')}</span>
            <div class="acp-stance">
              <button type="button" class="acp-stance-btn is-allow" data-tool-index="${i}" data-stance="allow"
                aria-pressed="${t.stance !== 'deny'}" ${disabled ? 'disabled' : ''}>Allow</button>
              <button type="button" class="acp-stance-btn is-block" data-tool-index="${i}" data-stance="deny"
                aria-pressed="${t.stance === 'deny'}" ${disabled ? 'disabled' : ''}>Block</button>
            </div>
          </div>`).join('')}
      </div>`;
  }

  #wireConnectors(list) {
    list.querySelectorAll('.acp-mcp-card').forEach((card) => {
      const connectorId = card.dataset.connector;
      card.querySelector('.acp-mcp-toggle-open').addEventListener('click', () => {
        if (this.#openConnectors.has(connectorId)) this.#openConnectors.delete(connectorId);
        else this.#openConnectors.add(connectorId);
        this.#renderConnectors();
      });
      card.querySelector('.acp-mcp-enable-input').addEventListener('change', (e) =>
        this.#setConnectorEnabled(connectorId, e.target.checked));
      card.querySelectorAll('.acp-stance-btn').forEach((btn) => {
        btn.addEventListener('click', () =>
          this.#setToolStance(connectorId, Number(btn.dataset.toolIndex), btn.dataset.stance));
      });
    });
  }

  async #setConnectorEnabled(connectorId, enabled) {
    const connector = this.#connectors.find((c) => c.connector_id === connectorId);
    try {
      await window.setAgentMcpConnectorAccess(this.#agent.id, connectorId, enabled);
      if (connector) connector.enabled = enabled;
    } catch (e) {
      showToast(`Failed to update access: ${e.message}`);
    }
    this.#renderConnectors();
  }

  // Applies one Allow/Block click by re-saving the connector's full rule set —
  // PUT /api/mcp/agents/{id}/tools replaces rules per connector mentioned in
  // the batch, so unmentioned connectors keep their rules.
  async #setToolStance(connectorId, toolIndex, stance) {
    const tools = this.#connectorTools.get(connectorId) || [];
    const tool = tools[toolIndex];
    if (!tool || tool.stance === stance) return;
    const previous = tool.stance;
    tool.stance = stance;
    this.#renderConnectors();
    try {
      const rules = tools.map((t) => ({
        connector_id: connectorId,
        tool_pattern: t.name,
        stance: t.stance === 'deny' ? 'deny' : 'allow',
      }));
      await window.saveAgentMcpToolRules(this.#agent.id, rules);
    } catch (e) {
      tool.stance = previous;
      this.#renderConnectors();
      showToast(`Failed to save tool rule: ${e.message}`);
    }
  }

  /* ── Settings tab ──────────────────────────────────────────────────────── */

  #settingsPanelHtml(a) {
    return `
        <div class="acp-panel" data-panel="settings">
          <section class="acp-section">
            <h2 class="acp-section-title">Agent identity</h2>
            <form class="acp-identity" id="acp-identity-form">
              <label class="acp-field">
                <span class="acp-field-label">Display name</span>
                <input type="text" id="acp-display-name" value="${this.#escAttr(a.display_name || a.name)}" maxlength="120" />
                <span class="acp-field-hint">Shown wherever this agent appears in Nasiko.</span>
              </label>
              <label class="acp-field">
                <span class="acp-field-label">Description</span>
                <textarea id="acp-description" rows="3">${this.#esc(a.description || '')}</textarea>
                <span class="acp-field-hint">Explain what this agent does for the people you share it with.</span>
              </label>
              <label class="acp-field">
                <span class="acp-field-label">Agent ID</span>
                <input type="text" value="${this.#escAttr(a.id)}" disabled />
                <span class="acp-field-hint">Generated when the agent was first published.</span>
              </label>
              <div class="acp-identity-actions">
                <button type="submit" class="acp-primary-btn">Save changes</button>
              </div>
            </form>
          </section>
          <section class="acp-section">
            <h2 class="acp-section-title">Agent Configuration</h2>
            <dl class="acp-dl">
              <div><dt>Image</dt><dd><code>${this.#esc(a.image || '—')}</code></dd></div>
              <div><dt>Port</dt><dd>${this.#esc(String(a.port || '—'))}</dd></div>
              <div><dt>Status</dt><dd><app-badge variant="${a.status === 'running' ? 'success' : a.status === 'error' ? 'error' : 'warning'}">${a.status || 'unknown'}</app-badge></dd></div>
              <div><dt>Replicas</dt><dd>${a.replicas ?? '—'}</dd></div>
            </dl>
          </section>
          <section class="acp-section">
            <secrets-manager id="acp-secrets" scope="agent" defer
              agent-id="${this.#escAttr(a.id)}"
              heading="Secrets"
              description="Environment secrets injected into this agent's container at deploy time. Values are write-only."></secrets-manager>
          </section>
          <section class="acp-section acp-danger">
            <h3 class="acp-danger-title">Danger zone</h3>
            <p class="acp-section-sub">Deleting this agent removes it from the registry, revokes all grants, and stops its container.</p>
            <button type="button" class="acp-action-btn acp-action-btn--danger" data-action="delete">${icons.trash('', 14)} Delete agent</button>
          </section>
        </div>`;
  }

  #wireSettings() {
    const identityForm = this.querySelector('#acp-identity-form');
    identityForm?.addEventListener('submit', (e) => this.#saveIdentity(e));
  }

  async #saveIdentity(e) {
    e.preventDefault();
    const displayName = this.querySelector('#acp-display-name').value.trim();
    const description = this.querySelector('#acp-description').value.trim();
    if (!displayName) {
      showToast('Display name cannot be empty');
      return;
    }
    try {
      await fetchApi(`/agents/${encodeURIComponent(this.#agent.id)}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ display_name: displayName, description }),
      });
    } catch (err) {
      showToast(`Failed to save: ${err.message}`);
      return;
    }
    this.#agent.display_name = displayName;
    this.#agent.description = description;
    document.title = `Nasiko — ${displayName}`;
    const h1 = this.querySelector('.acp-name');
    if (h1) h1.textContent = displayName;
    showToast('Agent details saved');
  }

  /* ── Overview stats ────────────────────────────────────────────────────── */

  async #loadStats() {
    const el = this.querySelector('#acp-stats');
    if (!el) return;

    // Renders in place of the skeletons. A section that can't answer must say
    // so — silently returning leaves four skeletons pulsing forever, which is
    // indistinguishable from a hung page.
    const unavailable = (description) => {
      el.innerHTML = `<div class="acp-stats-empty"><app-empty-state
        title="Metrics unavailable"
        description="${this.#escAttr(description)}"
        icon="${this.#escAttr(icons.trace('', 32))}"></app-empty-state></div>`;
    };

    let stats;
    try {
      // Same endpoint/shape as `nasiko observe stats`: {data:{project:{...}}}.
      const resp = await fetchApi(`/observability/agent/${this.#agentId}/stats`);
      stats = resp?.data?.project;
    } catch {
      // 503 when TEMPO_URL/LOKI_URL aren't configured, or any transport error.
      unavailable("The observability backend did not answer, so recent metrics can't be shown.");
      return;
    }

    if (!stats) {
      unavailable('The observability backend returned no data for this agent.');
      return;
    }

    const hasData = (stats.trace_count != null && stats.trace_count > 0) ||
                    (stats.cost_summary?.total?.cost > 0);

    if (!hasData) {
      el.innerHTML = `
        <div class="acp-stats-empty">
          <app-empty-state
            title="No usage data yet"
            description="Stats will appear after the first request to this agent."
            icon="${this.#escAttr(icons.trace('', 32))}">
          </app-empty-state>
        </div>`;
      return;
    }

    const fmtInt = (n) => n == null ? '—' : Number(n).toLocaleString();
    const fmtCost = (n) => {
      if (n == null) return '—';
      const v = +n;
      if (v === 0) return '$0';
      if (v < 0.01) return `$${v.toFixed(4)}`;
      return `$${v.toFixed(2)}`;
    };
    const fmtMs = (n) => n == null ? '—' : `${Math.round(n)} ms`;

    el.innerHTML = `
      <div class="acp-stat"><div class="acp-stat-label">Traces</div><div class="acp-stat-value">${fmtInt(stats.trace_count)}</div></div>
      <div class="acp-stat"><div class="acp-stat-label">Total cost</div><div class="acp-stat-value">${fmtCost(stats.cost_summary?.total?.cost)}</div></div>
      <div class="acp-stat"><div class="acp-stat-label">P50 latency</div><div class="acp-stat-value">${fmtMs(stats.latency_ms_p50)}</div></div>
      <div class="acp-stat"><div class="acp-stat-label">P99 latency</div><div class="acp-stat-value">${fmtMs(stats.latency_ms_p99)}</div></div>
    `;
  }

  /**
   * Container CPU / memory / network for this agent.
   *
   * Owner-scoped endpoint (`/observability/agent/{ref}/resources`), ACL-checked
   * server-side — deliberately not the admin whole-box endpoint, so this section
   * works for an agent's owner without exposing the rest of the host.
   */
  async #loadResourceUsage() {
    const el = this.querySelector('#acp-resources');
    if (!el) return;

    let usage;
    let state = '';
    try {
      const resp = await window.fetchAgentResourceStats(this.#agentId);
      usage = resp?.data?.usage ?? null;
      state = resp?.data?.usage?.state ?? '';
    } catch {
      // 503 on a runtime that cannot report usage, or 403 if access was revoked
      // mid-session. Either way there is nothing to show — say so rather than
      // leaving skeletons spinning forever.
      el.innerHTML = `<div class="acp-stats-empty"><app-empty-state
        title="Usage unavailable"
        description="Container resource usage could not be read for this agent."
        icon="${this.#escAttr(icons.cube('', 32))}"></app-empty-state></div>`;
      return;
    }

    // `usage: null` is the normal answer for an agent with no container right now
    // — scaled to zero or never deployed. Not an error.
    if (!usage) {
      el.innerHTML = `<div class="acp-stats-empty"><app-empty-state
        title="Not running"
        description="This agent has no running container, so there is nothing to measure."
        icon="${this.#escAttr(icons.cube('', 32))}"></app-empty-state></div>`;
      return;
    }

    const fmtBytes = (n) => {
      if (!n) return '0 B';
      const u = ['B', 'KB', 'MB', 'GB', 'TB'];
      let v = n;
      let i = 0;
      while (v >= 1024 && i < u.length - 1) { v /= 1024; i += 1; }
      return `${v >= 10 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${u[i]}`;
    };
    const meter = (pct) => {
      if (pct == null || Number.isNaN(pct)) return '';
      // Only the bar width is clamped — CPU legitimately exceeds 100% on
      // multi-core containers, and the label and severity must report that
      // rather than announce a capped 100%.
      const width = Math.max(0, Math.min(100, pct));
      const sev = pct >= 90 ? 'is-crit' : pct >= 70 ? 'is-warn' : 'is-ok';
      const word = pct >= 90 ? 'critical' : pct >= 70 ? 'high' : 'normal';
      return `<div class="acp-meter ${sev}" role="img" aria-label="${pct.toFixed(0)} percent, ${word}"><i style="width:${width.toFixed(1)}%"></i></div>`;
    };

    const cpuKnown = usage.cpu_percent != null;
    const memPct = usage.mem_limit_bytes
      ? (usage.mem_used_bytes / usage.mem_limit_bytes) * 100
      : null;

    el.innerHTML = `
      <div class="acp-stat">
        <div class="acp-stat-label">CPU</div>
        <div class="acp-stat-value">${cpuKnown ? `${usage.cpu_percent >= 10 ? usage.cpu_percent.toFixed(0) : usage.cpu_percent.toFixed(1)}%` : '—'}</div>
        ${cpuKnown ? meter(usage.cpu_percent) : '<div class="acp-stat-sub">not reporting</div>'}
        ${cpuKnown ? '<div class="acp-stat-sub">of one core</div>' : ''}
      </div>
      <div class="acp-stat">
        <div class="acp-stat-label">Memory</div>
        <div class="acp-stat-value">${fmtBytes(usage.mem_used_bytes)}</div>
        ${meter(memPct)}
        ${usage.mem_limit_bytes ? `<div class="acp-stat-sub">of ${fmtBytes(usage.mem_limit_bytes)}</div>` : ''}
      </div>
      <div class="acp-stat">
        <div class="acp-stat-label">Network</div>
        <div class="acp-stat-value">${fmtBytes(usage.net_rx_bytes)}</div>
        <div class="acp-stat-sub">in · ${fmtBytes(usage.net_tx_bytes)} out${state ? ` · ${this.#esc(state)}` : ''}</div>
      </div>
    `;
  }

  /* ── Logs tab ──────────────────────────────────────────────────────────── */

  #logsPanelHtml() {
    return `
        <div class="acp-panel" data-panel="logs">
          <section class="acp-section">
            <div class="acp-logs-toolbar">
              <div class="acp-logs-toolbar-start">
                <h2 class="acp-section-title">Container Logs</h2>
              </div>
              <div class="acp-logs-toolbar-end">
                <label class="acp-logs-tail-label">
                  Lines:
                  <select class="acp-logs-tail-select" id="acp-logs-tail">
                    <option value="50">50</option>
                    <option value="100" selected>100</option>
                    <option value="500">500</option>
                  </select>
                </label>
                <button class="acp-logs-follow-btn is-active" id="acp-logs-follow" title="Auto-scroll to latest logs">
                  ${icons.arrowDown('', 14)} Follow
                </button>
              </div>
            </div>
            <div class="acp-logs-viewer" id="acp-logs-viewer">
              <app-skeleton lines="12" height="320px"></app-skeleton>
            </div>
          </section>
        </div>`;
  }

  #wireLogsControls() {
    const tailSelect = this.querySelector('#acp-logs-tail');
    tailSelect?.addEventListener('change', () => {
      this.#logsTail = Number(tailSelect.value);
      this.#logsLoaded = false;
      this.#loadLogs();
    });
    const followBtn = this.querySelector('#acp-logs-follow');
    followBtn?.addEventListener('click', () => {
      this.#logsFollowing = !this.#logsFollowing;
      followBtn.classList.toggle('is-active', this.#logsFollowing);
      if (this.#logsFollowing) this.#scrollLogsToBottom();
    });
  }

  async #loadLogs() {
    const viewer = this.querySelector('#acp-logs-viewer');
    if (!viewer) return;

    if (!this.#logsLoaded) {
      viewer.innerHTML = '<app-skeleton lines="12" height="320px"></app-skeleton>';
    }

    try {
      const logs = await fetchApi(`/observability/agents/${this.#agentId}/logs?limit=${this.#logsTail}`);
      this.#logsLoaded = true;

      if (!logs || logs.length === 0) {
        viewer.innerHTML = `
          <div class="acp-logs-empty">
            <app-empty-state
              title="No logs available"
              description="This agent has not produced any log output yet.">
            </app-empty-state>
          </div>`;
        return;
      }

      const linesHtml = logs.map((line, i) => {
        const ts = this.#formatLogTimestamp(line.timestamp);
        return `<div class="acp-log-line">` +
          `<span class="acp-log-num">${i + 1}</span>` +
          `<span class="acp-log-ts">${this.#esc(ts)}</span>` +
          `<app-badge class="acp-log-badge" variant="${this.#levelVariant(line.level)}">${this.#esc(line.level || 'info')}</app-badge>` +
          `<span class="acp-log-msg">${ansiToHtml(line.message)}</span>` +
          `</div>`;
      }).join('');

      viewer.innerHTML = `<div class="acp-logs-scroll">${linesHtml}</div>`;

      if (this.#logsFollowing) {
        this.#scrollLogsToBottom();
      }
    } catch {
      this.#logsLoaded = false;
      viewer.innerHTML = `
        <div class="acp-logs-empty">
          <app-empty-state
            title="Failed to load logs"
            description="Could not fetch logs for this agent. The agent may not be running.">
          </app-empty-state>
        </div>`;
    }
  }

  #scrollLogsToBottom() {
    const scroll = this.querySelector('.acp-logs-scroll');
    if (scroll) {
      scroll.scrollTop = scroll.scrollHeight;
    }
  }

  #formatLogTimestamp(ts) {
    if (!ts) return '';
    const d = new Date(ts);
    if (Number.isNaN(d.getTime())) return ts;
    return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  #levelVariant(level) {
    switch (level) {
      case 'error': return 'error';
      case 'warn': return 'warning';
      case 'debug': return 'neutral';
      default: return 'success';
    }
  }

  /* ── Utilities ─────────────────────────────────────────────────────────── */

  #debounce(fn, ms) {
    let timer = null;
    return (...args) => {
      clearTimeout(timer);
      timer = setTimeout(() => fn(...args), ms);
    };
  }

  #shortId(id) {
    const s = String(id || '');
    return s.length > 12 ? `${s.slice(0, 8)}…` : s;
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }

  #escAttr(s) {
    return (s || '').replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }
}

customElements.define('agent-card-page', AgentCardPage);
