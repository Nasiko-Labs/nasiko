import { icons } from '/common/utils/icons.js';
import { fetchApi } from '/common/services/api.js';
import styles from './agent-card-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TABS = [
  { key: 'overview', label: 'Overview', icon: 'layers' },
  { key: 'settings', label: 'Settings', icon: 'settings' },
];

class AgentCardPage extends HTMLElement {
  #agent = null;
  #agentId = null;

  connectedCallback() {
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
      this.#agent = await fetchApi(`/catalog/agents/${this.#agentId}`);
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
    const tagsHtml = (a.tags || []).slice(0, 3).map(t =>
      `<span class="acp-tag">${this.#esc(t)}</span>`
    ).join('');
    const extraTagCount = (a.tags || []).length - 3;
    const moreTag = extraTagCount > 0 ? `<span class="acp-tag acp-tag--more">+${extraTagCount}</span>` : '';

    const skills = a.skills || [];
    const skillsHtml = skills.map(s => `
      <div class="acp-skill-card">
        <div class="acp-skill-name">${this.#esc(s.name)}</div>
        <div class="acp-skill-desc">${this.#esc(s.description || '')}</div>
        ${s.sample_query ? `
        <div class="acp-skill-sample">
          <span class="acp-skill-sample-icon">${icons.send('', 14)}</span>
          <span class="acp-skill-sample-text">Sample query: ${this.#esc(s.sample_query)}</span>
          <a class="acp-skill-sample-go" href="/chat.html?agent_id=${encodeURIComponent(a.id)}&query=${encodeURIComponent(s.sample_query)}">${icons.externalLink('', 14)}</a>
        </div>` : ''}
      </div>
    `).join('');

    const caps = a.capabilities || {};

    this.innerHTML = `
      <div class="acp-page">
        <div class="acp-header">
          <button class="acp-back" onclick="history.back()" title="Go back">${icons.chevronLeft('', 20)}</button>
          <div class="acp-header-main">
            <div class="acp-header-title">
              <h1 class="acp-name">${this.#esc(a.display_name || a.name)}</h1>
              <span class="acp-version">v${this.#esc(a.version || '?')}</span>
            </div>
            <div class="acp-header-tags">${tagsHtml}${moreTag}</div>
          </div>
          <div class="acp-header-actions">
            ${a.status === 'running' ? `<button class="acp-action-btn" data-action="stop" title="Stop agent">${icons.square('', 14)} Stop</button>` : ''}
            <button class="acp-action-btn acp-action-btn--danger" data-action="delete" title="Delete agent">${icons.trash('', 14)} Delete</button>
            <a class="acp-start-btn" href="/chat.html?agent_id=${encodeURIComponent(a.id)}&agent_name=${encodeURIComponent(a.display_name || a.name)}">
              Start session ${icons.send('', 16)}
            </a>
          </div>
        </div>

        <p class="acp-description">${this.#esc(a.description || '')}</p>

        <nav class="acp-tabs">
          ${TABS.map(t => `<button class="acp-tab${t.key === 'overview' ? ' is-active' : ''}" data-tab="${t.key}">${t.label}</button>`).join('')}
        </nav>

        <div class="acp-panel is-active" data-panel="overview">
          ${skills.length ? `
          <section class="acp-section">
            <h2 class="acp-section-title">Skills</h2>
            <p class="acp-section-sub">What this agent can do. Click a capability to auto-fill a query.</p>
            <div class="acp-skills-grid">${skillsHtml}</div>
          </section>` : ''}

          <section class="acp-section">
            <h2 class="acp-section-title">Quick performance</h2>
            <p class="acp-section-sub">Recent runtime metrics for this agent.</p>
            <div class="acp-stats-grid" id="acp-stats">
              <div class="acp-stat"><div class="acp-stat-label">Total executions</div><div class="acp-stat-value">—</div></div>
              <div class="acp-stat"><div class="acp-stat-label">Total cost</div><div class="acp-stat-value">—</div></div>
              <div class="acp-stat"><div class="acp-stat-label">P50 latency</div><div class="acp-stat-value">—</div></div>
              <div class="acp-stat"><div class="acp-stat-label">P99 latency</div><div class="acp-stat-value">—</div></div>
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
      </div>
    `;

    this.querySelector('.acp-tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.acp-tab');
      if (!tab) return;
      this.querySelectorAll('.acp-tab').forEach(t => t.classList.remove('is-active'));
      this.querySelectorAll('.acp-panel').forEach(p => p.classList.remove('is-active'));
      tab.classList.add('is-active');
      this.querySelector(`[data-panel="${tab.dataset.tab}"]`).classList.add('is-active');
    });

    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('[data-action]');
      if (!btn) return;
      const action = btn.dataset.action;

      if (action === 'stop') {
        try {
          const res = await fetch(`/api/containers/${encodeURIComponent(a.name)}/stop`, { method: 'POST' });
          if (!res.ok) throw new Error(await res.text());
          location.reload();
        } catch (err) {
          alert(`Failed to stop: ${err.message}`);
        }
      } else if (action === 'delete') {
        if (!confirm(`Delete "${a.display_name || a.name}"? This removes the agent from the registry and stops its container.`)) return;
        try {
          const res = await fetch(`/api/catalog/agents/${encodeURIComponent(a.id)}`, { method: 'DELETE' });
          if (!res.ok) throw new Error(await res.text());
          location.href = '/your-agents.html';
        } catch (err) {
          alert(`Failed to delete: ${err.message}`);
        }
      }
    });

    this.#loadStats();
  }

  async #loadStats() {
    try {
      const s = await fetchApi(`/observe/agents/${this.#agentId}/stats`);
      const el = this.querySelector('#acp-stats');
      if (!el || !s) return;
      const fmt = (n, dec = 0) => (n == null ? '—' : (+n).toFixed(dec));
      el.innerHTML = `
        <div class="acp-stat"><div class="acp-stat-label">Total executions</div><div class="acp-stat-value">${fmt(s.total_requests)}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">Total cost</div><div class="acp-stat-value">$${fmt(s.total_cost || 0, 5)}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">P50 latency</div><div class="acp-stat-value">${s.p50_latency_ms != null ? fmt(s.p50_latency_ms) + ' ms' : '—'}</div></div>
        <div class="acp-stat"><div class="acp-stat-label">P99 latency</div><div class="acp-stat-value">${s.p95_latency_ms != null ? fmt(s.p95_latency_ms) + ' ms' : '—'}</div></div>
      `;
    } catch {}
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
