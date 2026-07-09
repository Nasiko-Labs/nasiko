import { icons } from '/common/utils/icons.js';
import { fetchApi, apiFetch } from '/common/services/api.js';
import { showToast } from '/common/utils/toast.js';
import styles from './agent-card-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TABS = [
  { key: 'overview', label: 'Overview', icon: 'layers' },
  { key: 'settings', label: 'Settings', icon: 'settings' },
  { key: 'logs', label: 'Logs', icon: 'terminal' },
];

/** Deterministic color from a string hash, mapped to pleasant palette. */
function avatarColor(name) {
  const colors = [
    'var(--color-primary)',
    'var(--color-success)',
    'var(--color-warning)',
    'var(--color-error)',
    'var(--color-info)',
  ];
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = ((hash << 5) - hash + name.charCodeAt(i)) | 0;
  }
  return colors[Math.abs(hash) % colors.length];
}

class AgentCardPage extends HTMLElement {
  #initialized = false;
  #agent = null;
  #agentId = null;
  #logsLoaded = false;
  #logsTail = 100;
  #logsFollowing = true;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#agentId = new URLSearchParams(location.search).get('id');
    if (!this.#agentId) {
      this.innerHTML = '<p style="color:var(--color-text-muted);">No agent ID specified.</p>';
      return;
    }
    this.innerHTML = '<app-skeleton height="400px" style="max-width:900px;margin:0 auto;"></app-skeleton>';
    this.#load();
  }

  async #load() {
    try {
      this.#agent = await fetchApi(`/agents/${this.#agentId}`);
    } catch {
      this.#agent = null;
    }
    if (!this.#agent) {
      this.innerHTML = '<p style="color:var(--color-error);">Agent not found.</p>';
      return;
    }
    document.title = `Nasiko — ${this.#agent.display_name || this.#agent.name}`;
    this.#render();
  }

  #render() {
    const a = this.#agent;
    const displayName = a.display_name || a.name;
    const initial = (displayName || '?')[0].toUpperCase();
    const bgColor = avatarColor(displayName);

    const tagsHtml = (a.tags || []).slice(0, 3).map(t =>
      `<span class="acp-tag">${this.#esc(t)}</span>`
    ).join('');
    const extraTagCount = (a.tags || []).length - 3;
    const moreTag = extraTagCount > 0 ? `<span class="acp-tag acp-tag--more">+${extraTagCount}</span>` : '';

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
        <div class="acp-header">
          <a class="acp-back" href="/your-agents.html" title="Back to Your Agents">
            ${icons.chevronLeft('', 18)}
            <span class="acp-back-text">Your Agents</span>
          </a>
          <div class="acp-header-main">
            <div class="acp-header-title">
              <div class="acp-avatar" style="background:${bgColor}">${initial}</div>
              <h1 class="acp-name">${this.#esc(displayName)}</h1>
              <span class="acp-version">v${this.#esc(a.version || '?')}</span>
            </div>
            <div class="acp-header-tags">${tagsHtml}${moreTag}</div>
          </div>
          <div class="acp-header-actions">
            ${a.status === 'running' ? `<button class="acp-action-btn" data-action="restart" title="Restart agent">${icons.refresh('', 14)} Restart</button>` : ''}
            ${a.status === 'running' ? `<button class="acp-action-btn" data-action="stop" title="Stop agent">${icons.square('', 14)} Stop</button>` : ''}
            <button class="acp-action-btn acp-action-btn--danger" data-action="delete" title="Delete agent">${icons.trash('', 14)} Delete</button>
            <a class="acp-start-btn" href="/chat.html?agent_id=${encodeURIComponent(a.id)}&agent_name=${encodeURIComponent(displayName)}">
              Start session ${icons.send('', 16)}
            </a>
          </div>
        </div>

        <p class="acp-description">${this.#esc(a.description || '')}</p>

        <nav class="acp-tabs">
          ${TABS.map(t => `<button class="acp-tab${t.key === 'overview' ? ' is-active' : ''}" data-tab="${t.key}">${icons[t.icon]('acp-tab-icon', 16)} ${t.label}</button>`).join('')}
        </nav>

        <div class="acp-panel is-active" data-panel="overview">
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
    });

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
      const s = await fetchApi(`/observability/agent/${this.#agentId}/stats`);
      const el = this.querySelector('#acp-stats');
      if (!el || !s) return;

      const hasData = (s.total_requests != null && s.total_requests > 0) ||
                      (s.total_cost != null && s.total_cost > 0);

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
        <div class="acp-stat"><div class="acp-stat-label">Total executions</div><div class="acp-stat-value">${fmtInt(s.total_requests)}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">Total cost</div><div class="acp-stat-value">${fmtCost(s.total_cost)}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">P50 latency</div><div class="acp-stat-value">${fmtMs(s.p50_latency_ms)}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">P99 latency</div><div class="acp-stat-value">${fmtMs(s.p95_latency_ms)}</div></div>
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

  async #loadLogs() {
    const viewer = this.querySelector('#acp-logs-viewer');
    if (!viewer) return;

    if (!this.#logsLoaded) {
      viewer.innerHTML = '<app-skeleton lines="12" height="320px"></app-skeleton>';
    }

    try {
      const logs = await fetchApi(`/observability/agents/${this.#agentId}/logs?tail=${this.#logsTail}`);
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
          `<span class="acp-log-msg">${this.#esc(line.message)}</span>` +
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
