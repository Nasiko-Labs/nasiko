import { icons } from '/common/utils/icons.js';
import { fetchApi, apiFetch } from '/common/services/api.js';
import { showToast } from '/common/utils/toast.js';
import { ansiToHtml } from '/common/utils/ansi.js';
import styles from './agent-card-page.css' with { type: 'css' };
import '/common/components/app-empty-state.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TABS = [
  { key: 'overview', label: 'Overview' },
  { key: 'settings', label: 'Settings' },
  { key: 'logs', label: 'Logs' },
];

class AgentCardPage extends HTMLElement {
  #initialized = false;
  #agent = null;
  #agentId = null;
  #logsLoaded = false;
  #secretsLoaded = false;
  #logsTail = 100;
  #logsFollowing = true;

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
    document.title = `Nasiko — ${this.#agent.display_name || this.#agent.name}`;
    this.#render();
  }

  #render() {
    const a = this.#agent;
    const displayName = a.display_name || a.name;

    const tagsHtml = (a.tags || []).slice(0, 3).map(t =>
      `<span class="acp-tag">${this.#esc(t)}</span>`
    ).join('');
    const extraTagCount = (a.tags || []).length - 3;
    const moreTag = extraTagCount > 0 ? `<span class="acp-tag acp-tag--more">+${extraTagCount}</span>` : '';

    const statusVariant = a.status === 'running' ? 'success' : (a.status === 'error' || a.status === 'failed') ? 'error' : 'neutral';
    const statusLabel = a.status === 'running' ? 'Active' : (a.status || 'Unknown').replace(/^./, c => c.toUpperCase());

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

    this.innerHTML = `
      <div class="acp-page">
        <div class="acp-topbar">
          <a class="acp-back" href="/your-agents.html" title="Back to Your Agents" aria-label="Back to Your Agents">
            ${icons.x('', 16)}
          </a>
          <div class="acp-topbar-actions">
            ${a.status === 'running' ? `<button class="acp-action-btn" data-action="restart" title="Restart agent">${icons.refresh('', 14)} Restart</button>` : ''}
            ${a.status === 'running' ? `<button class="acp-action-btn" data-action="stop" title="Stop agent">${icons.square('', 14)} Stop</button>` : ''}
            <button class="acp-action-btn acp-action-btn--danger" data-action="delete" title="Delete agent">${icons.trash('', 14)} Delete</button>
          </div>
        </div>

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
        </div>

        <nav class="acp-tabs">
          ${TABS.map(t => `<button class="acp-tab${t.key === 'overview' ? ' is-active' : ''}" data-tab="${t.key}">${t.label}</button>`).join('')}
        </nav>

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

          <section class="acp-section" id="acp-acl">
            <h2 class="acp-section-title">${icons.key('acp-acl-icon', 16)} Access Control</h2>
            <p class="acp-section-sub">Which agents and users can interact with this agent.</p>
            <div class="acp-acl-content">
              <app-skeleton height="80px"></app-skeleton>
            </div>
          </section>
          </div>
        </div>

        <div class="acp-panel" data-panel="settings">
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
            <h2 class="acp-section-title">Secrets</h2>
            <p class="acp-section-sub">Environment secrets injected into this agent's container at deploy time. Values are write-only.</p>
            <div id="acp-secrets-list"><app-skeleton height="60px"></app-skeleton></div>
            <form class="acp-secret-form" id="acp-secret-form">
              <input type="text" id="acp-secret-name" placeholder="SECRET_NAME"
                pattern="[A-Za-z_][A-Za-z0-9_]*" autocomplete="off" required />
              <input type="password" id="acp-secret-value" placeholder="Value" autocomplete="off" required />
              <button type="submit" class="acp-action-btn">${icons.plus('', 14)} Set secret</button>
            </form>
          </section>
        </div>

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
        </div>
      </div>
    `;

    this.querySelector('.acp-tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.acp-tab');
      if (!tab) return;
      this.querySelectorAll('.acp-tab').forEach(t => t.classList.remove('is-active'));
      this.querySelectorAll('.acp-panel').forEach(p => p.classList.remove('is-active'));
      tab.classList.add('is-active');
      this.querySelector(`[data-panel="${tab.dataset.tab}"]`).classList.add('is-active');
      if (tab.dataset.tab === 'logs' && !this.#logsLoaded) {
        this.#loadLogs();
      }
      if (tab.dataset.tab === 'settings' && !this.#secretsLoaded) {
        this.#loadSecrets();
      }
    });

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

    const secretForm = this.querySelector('#acp-secret-form');
    if (secretForm) {
      secretForm.addEventListener('submit', (e) => this.#setSecret(e));
    }

    const tailSelect = this.querySelector('#acp-logs-tail');
    if (tailSelect) {
      tailSelect.addEventListener('change', () => {
        this.#logsTail = Number(tailSelect.value);
        this.#logsLoaded = false;
        this.#loadLogs();
      });
    }

    const followBtn = this.querySelector('#acp-logs-follow');
    if (followBtn) {
      followBtn.addEventListener('click', () => {
        this.#logsFollowing = !this.#logsFollowing;
        followBtn.classList.toggle('is-active', this.#logsFollowing);
        if (this.#logsFollowing) {
          this.#scrollLogsToBottom();
        }
      });
    }

    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('[data-action]');
      if (!btn) return;
      const action = btn.dataset.action;

      if (action === 'restart') {
        const original = btn.innerHTML;
        btn.disabled = true;
        btn.textContent = 'Restarting...';
        try {
          const res = await apiFetch(`/containers/${encodeURIComponent(a.name)}/restart`, { method: 'POST' });
          if (!res.ok) throw new Error(await res.text());
          showToast('Agent restarted');
          location.reload();
        } catch (err) {
          showToast(`Failed to restart: ${err.message}`);
          btn.disabled = false;
          btn.innerHTML = original;
        }
      } else if (action === 'stop') {
        const original = btn.innerHTML;
        btn.disabled = true;
        btn.textContent = 'Stopping...';
        try {
          const res = await apiFetch(`/containers/${encodeURIComponent(a.name)}/stop`, { method: 'POST' });
          if (!res.ok) throw new Error(await res.text());
          location.reload();
        } catch (err) {
          showToast(`Failed to stop: ${err.message}`);
          btn.disabled = false;
          btn.innerHTML = original;
        }
      } else if (action === 'delete') {
        if (!confirm(`Delete "${displayName}"? This removes the agent from the registry and stops its container.`)) return;
        const original = btn.innerHTML;
        btn.disabled = true;
        btn.textContent = 'Deleting...';
        try {
          const res = await apiFetch(`/agents/${encodeURIComponent(a.id)}`, { method: 'DELETE' });
          if (!res.ok) throw new Error(await res.text());
          location.href = '/your-agents.html';
        } catch (err) {
          showToast(`Failed to delete: ${err.message}`);
          btn.disabled = false;
          btn.innerHTML = original;
        }
      }
    });

    this.#loadStats();
    this.#loadAcl();
  }

  async #loadStats() {
    try {
      // Same endpoint/shape as `nasiko observe stats`: {data:{project:{...}}}.
      const resp = await fetchApi(`/observability/agent/${this.#agentId}/stats`);
      const s = resp?.data?.project;
      const el = this.querySelector('#acp-stats');
      if (!el || !s) return;

      const hasData = (s.trace_count != null && s.trace_count > 0) ||
                      (s.cost_summary?.total?.cost > 0);

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
        <div class="acp-stat"><div class="acp-stat-label">Traces</div><div class="acp-stat-value">${fmtInt(s.trace_count)}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">Total cost</div><div class="acp-stat-value">${fmtCost(s.cost_summary?.total?.cost)}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">P50 latency</div><div class="acp-stat-value">${fmtMs(s.latency_ms_p50)}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">P99 latency</div><div class="acp-stat-value">${fmtMs(s.latency_ms_p99)}</div></div>
      `;
    } catch {
      /* stats are optional — fail silently */
    }
  }

  async #loadAcl() {
    const el = this.querySelector('.acp-acl-content');
    if (!el) return;

    try {
      // Fetch agent-to-agent ACL (OSS: allowed targets this agent can call)
      const [acl, visibility] = await Promise.all([
        fetchApi(`/agents/${this.#agentId}/acl`).catch(() => null),
        fetchApi(`/agents/${this.#agentId}/visibility`).catch(() => null),
      ]);

      const sections = [];

      // Agent-to-agent section (OSS)
      if (acl) {
        const targets = acl.allowed || [];
        if (targets.length > 0) {
          const badges = targets.map(id =>
            `<app-badge variant="info">${this.#esc(id)}</app-badge>`
          ).join(' ');
          sections.push(`
            <div class="acp-acl-group">
              <h3 class="acp-acl-group-title">${icons.send('acp-acl-group-icon', 14)} Can invoke</h3>
              <p class="acp-acl-group-desc">Agents this one is allowed to call.</p>
              <div class="acp-acl-badges">${badges}</div>
            </div>
          `);
        } else {
          sections.push(`
            <div class="acp-acl-group">
              <h3 class="acp-acl-group-title">${icons.send('acp-acl-group-icon', 14)} Can invoke</h3>
              <p class="acp-acl-group-desc">No outbound agent-to-agent permissions configured. This agent cannot call other agents.</p>
            </div>
          `);
        }
      }

      // EE: visibility/grants (user, team, department, public)
      if (visibility) {
        const grants = visibility.grants || [];
        const isPublic = visibility.is_public;

        if (isPublic) {
          sections.push(`
            <div class="acp-acl-group">
              <h3 class="acp-acl-group-title">${icons.eye('acp-acl-group-icon', 14)} Visibility</h3>
              <div class="acp-acl-badges"><app-badge variant="success">Public</app-badge></div>
            </div>
          `);
        } else {
          const userGrants = grants.filter(g => g.grant_type === 'user');
          const teamGrants = grants.filter(g => g.grant_type === 'team');
          const deptGrants = grants.filter(g => g.grant_type === 'department');
          const agentGrants = grants.filter(g => g.grant_type === 'agent');

          const parts = [];
          if (userGrants.length > 0) {
            parts.push(`<app-badge variant="neutral">${userGrants.length} user${userGrants.length > 1 ? 's' : ''}</app-badge>`);
          }
          if (teamGrants.length > 0) {
            parts.push(`<app-badge variant="info">${teamGrants.length} team${teamGrants.length > 1 ? 's' : ''}</app-badge>`);
          }
          if (deptGrants.length > 0) {
            parts.push(`<app-badge variant="warning">${deptGrants.length} department${deptGrants.length > 1 ? 's' : ''}</app-badge>`);
          }
          if (agentGrants.length > 0) {
            parts.push(`<app-badge variant="info">${agentGrants.length} agent${agentGrants.length > 1 ? 's' : ''}</app-badge>`);
          }

          if (parts.length > 0) {
            sections.push(`
              <div class="acp-acl-group">
                <h3 class="acp-acl-group-title">${icons.users('acp-acl-group-icon', 14)} Granted access</h3>
                <p class="acp-acl-group-desc">Who can interact with this agent.</p>
                <div class="acp-acl-badges">${parts.join(' ')}</div>
              </div>
            `);
          } else {
            sections.push(`
              <div class="acp-acl-group">
                <h3 class="acp-acl-group-title">${icons.lock('acp-acl-group-icon', 14)} Visibility</h3>
                <p class="acp-acl-group-desc">Private. Only the owner has access.</p>
                <div class="acp-acl-badges"><app-badge variant="warning">Private</app-badge></div>
              </div>
            `);
          }
        }
      }

      if (sections.length === 0) {
        el.innerHTML = `
          <div class="acp-acl-empty">
            <app-empty-state
              title="No access data"
              description="Access control information is not available for this agent.">
            </app-empty-state>
          </div>`;
        return;
      }

      el.innerHTML = `<div class="acp-acl-grid">${sections.join('')}</div>`;
    } catch {
      el.innerHTML = '';
    }
  }

  /* ── Agent secrets (settings tab) ─────────────────────────────────────── */

  async #loadSecrets() {
    const list = this.querySelector('#acp-secrets-list');
    if (!list) return;
    this.#secretsLoaded = true;
    let secrets = [];
    try {
      // GET /api/agents/{id}/secrets → [SecretListEntry {name, updated_at?}]
      secrets = await fetchApi(`/agents/${encodeURIComponent(this.#agent.id)}/secrets`);
    } catch {
      list.innerHTML = '<p class="acp-secrets-empty">You need owner access to manage this agent\'s secrets.</p>';
      return;
    }
    this.#renderSecrets(Array.isArray(secrets) ? secrets : []);
  }

  #renderSecrets(secrets) {
    const list = this.querySelector('#acp-secrets-list');
    if (!list) return;
    if (!secrets.length) {
      list.innerHTML = '<p class="acp-secrets-empty">No secrets configured for this agent yet.</p>';
      return;
    }
    list.innerHTML = `
      <ul class="acp-secrets">
        ${secrets.map((s) => `
          <li class="acp-secret-row">
            <span class="acp-secret-name">${icons.lock('', 13)} ${this.#esc(s.name)}</span>
            <span class="acp-secret-value">••••••••</span>
            <button type="button" class="acp-secret-delete" data-secret-name="${this.#esc(s.name)}"
              aria-label="Delete secret ${this.#esc(s.name)}">${icons.trash('', 14)}</button>
          </li>`).join('')}
      </ul>
    `;
    list.querySelectorAll('.acp-secret-delete').forEach((btn) => {
      btn.addEventListener('click', () => this.#deleteSecret(btn.dataset.secretName));
    });
  }

  async #setSecret(e) {
    e.preventDefault();
    const nameInput = this.querySelector('#acp-secret-name');
    const valueInput = this.querySelector('#acp-secret-value');
    const name = nameInput.value.trim();
    try {
      const res = await apiFetch(`/agents/${encodeURIComponent(this.#agent.id)}/secrets`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name, value: valueInput.value }),
      });
      if (!res.ok) throw new Error(await res.text());
    } catch (err) {
      showToast(`Failed to set secret: ${err.message}`);
      return;
    }
    nameInput.value = '';
    valueInput.value = '';
    showToast(`Secret ${name} saved — restart the agent to apply`);
    this.#secretsLoaded = false;
    this.#loadSecrets();
  }

  async #deleteSecret(name) {
    if (!confirm(`Delete secret ${name}?`)) return;
    try {
      const res = await apiFetch(
        `/agents/${encodeURIComponent(this.#agent.id)}/secrets/${encodeURIComponent(name)}`,
        { method: 'DELETE' },
      );
      if (!res.ok) throw new Error(await res.text());
    } catch (err) {
      showToast(`Failed to delete secret: ${err.message}`);
      return;
    }
    showToast(`Secret ${name} deleted`);
    this.#secretsLoaded = false;
    this.#loadSecrets();
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
