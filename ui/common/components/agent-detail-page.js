import { icons } from '/common/utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (agent-detail-page) {
  :scope { display: block; }
  .header {
    display: flex;
    align-items: center;
    gap: var(--space-md);
    margin-bottom: var(--space-xl);
  }
  .header-info { flex: 1; }
  .header-name {
    font-size: var(--font-size-2xl);
    font-weight: 600;
    color: var(--color-text-main);
  }
  .header-meta {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin-top: var(--space-xs);
    display: flex;
    gap: var(--space-md);
    align-items: center;
  }
  .header-actions {
    display: flex;
    gap: var(--space-sm);
  }
  .action-btn {
    padding: var(--space-xs) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    cursor: pointer;
  }
  .action-btn:hover { border-color: var(--color-primary); }
  .action-btn.is-primary {
    border-color: var(--color-primary);
    background: color-mix(in srgb, var(--color-primary) 10%, transparent);
  }
  .tabs {
    display: flex;
    gap: var(--space-md);
    border-bottom: 1px solid var(--color-border);
    margin-bottom: var(--space-lg);
  }
  .tab {
    padding: var(--space-sm) var(--space-xs);
    font-size: var(--font-size-sm);
    font-weight: 500;
    color: var(--color-text-muted);
    border: none;
    border-bottom: 2px solid transparent;
    background: none;
    cursor: pointer;
  }
  .tab:hover { color: var(--color-text-main); }
  .tab.is-active { color: var(--color-primary); border-bottom-color: var(--color-primary); }
  .panel { display: none; }
  .panel.is-active { display: block; }
  .info-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: var(--space-md);
    margin-bottom: var(--space-xl);
  }
  .info-card {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    background: var(--color-bg-surface);
  }
  .info-label {
    font-size: var(--font-size-xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--color-text-muted);
  }
  .info-value {
    font-size: var(--font-size-base);
    color: var(--color-text-main);
    margin-top: var(--space-xs);
    font-weight: 500;
  }
  .tags { display: flex; flex-wrap: wrap; gap: 6px; margin-top: var(--space-sm); }
  .tag {
    font-size: var(--font-size-xs);
    padding: 2px 10px;
    border-radius: var(--radius-sm);
    background: var(--color-bg-base);
    border: 1px solid var(--color-border);
    color: var(--color-text-muted);
  }
  .skills-list {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: var(--space-sm);
  }
  .skill-item {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-sm) var(--space-md);
    background: var(--color-bg-surface);
  }
  .skill-name { font-weight: 500; font-size: var(--font-size-sm); }
  .skill-desc { font-size: var(--font-size-xs); color: var(--color-text-muted); margin-top: 2px; }
  .log-viewer {
    background: var(--color-bg-base);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    font-family: var(--font-mono);
    font-size: var(--font-size-xs);
    line-height: 1.8;
    max-height: 400px;
    overflow-y: auto;
    white-space: pre-wrap;
    word-break: break-all;
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TABS = [
  { key: 'overview', label: 'Overview' },
  { key: 'skills', label: 'Skills' },
  { key: 'logs', label: 'Logs' },
  { key: 'flows', label: 'Flows' },
];

class AgentDetailPage extends HTMLElement {
  #agent = null;
  #agentId = null;

  connectedCallback() {
    this.#agentId = new URLSearchParams(location.search).get('id');
    if (!this.#agentId) {
      this.innerHTML = '<p style="color:var(--color-text-muted);">No agent ID specified.</p>';
      return;
    }
    this.innerHTML = '<app-skeleton height="200px"></app-skeleton>';
    this.#load();
  }

  async #load() {
    this.#agent = await window.fetchAgentDetail(this.#agentId);
    if (!this.#agent) {
      this.innerHTML = '<p style="color:var(--color-error);">Agent not found.</p>';
      return;
    }
    document.title = `Nasiko — ${this.#agent.display_name || this.#agent.name}`;
    this.#render();
  }

  #render() {
    const a = this.#agent;
    this.innerHTML = `
      <div class="header">
        <div class="header-info">
          <div class="header-name">${a.display_name || a.name}</div>
          <div class="header-meta">
            <code style="font-size:var(--font-size-xs);background:var(--color-bg-base);padding:2px 6px;border-radius:var(--radius-sm);">${a.image || ''}</code>
            <span>v${a.version || '?'}</span>
            <app-badge variant="${a.status === 'running' ? 'success' : 'warning'}">${a.status || 'unknown'}</app-badge>
          </div>
        </div>
        <div class="header-actions">
          <a class="action-btn is-primary" href="/chat.html?agent_id=${a.id}">Chat</a>
          <a class="action-btn" href="/agent-card.html?id=${a.id}">View Card</a>
        </div>
      </div>

      <nav class="tabs">${TABS.map(t =>
        `<button class="tab${t.key === 'overview' ? ' is-active' : ''}" data-tab="${t.key}">${t.label}</button>`
      ).join('')}</nav>

      <div class="panel is-active" data-panel="overview">
        <div class="info-grid">
          <div class="info-card"><div class="info-label">Description</div><div class="info-value" style="font-weight:400;">${a.description || '—'}</div></div>
          <div class="info-card"><div class="info-label">Status</div><div class="info-value"><app-badge variant="${a.status === 'running' ? 'success' : 'warning'}">${a.status}</app-badge></div></div>
          <div class="info-card"><div class="info-label">Port</div><div class="info-value">${a.port || '—'}</div></div>
          <div class="info-card"><div class="info-label">Version</div><div class="info-value">${a.version || '—'}</div></div>
        </div>
        ${a.tags?.length ? `<div class="tags">${a.tags.map(t => `<span class="tag">${t}</span>`).join('')}</div>` : ''}
      </div>

      <div class="panel" data-panel="skills">
        <div class="skills-list" id="skills-list">
          ${(a.skills || []).map(s => `
            <div class="skill-item">
              <div class="skill-name">${s.name}</div>
              <div class="skill-desc">${s.description || ''}</div>
            </div>
          `).join('') || '<p style="color:var(--color-text-muted);font-style:italic;">No skills registered.</p>'}
        </div>
      </div>

      <div class="panel" data-panel="logs">
        <div class="log-viewer" id="log-viewer">Loading logs...</div>
      </div>

      <div class="panel" data-panel="flows">
        <p style="color:var(--color-text-muted);">Flow traces involving this agent will appear here.</p>
      </div>
    `;

    this.querySelector('.tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.tab');
      if (!tab) return;
      this.querySelectorAll('.tab').forEach(t => t.classList.remove('is-active'));
      this.querySelectorAll('.panel').forEach(p => p.classList.remove('is-active'));
      tab.classList.add('is-active');
      this.querySelector(`[data-panel="${tab.dataset.tab}"]`).classList.add('is-active');
      if (tab.dataset.tab === 'logs') this.#loadLogs();
    });
  }

  async #loadLogs() {
    const viewer = this.querySelector('#log-viewer');
    const result = await window.fetchAgentLogs(this.#agentId, '', 1, 50);
    if (!result.data.length) {
      viewer.textContent = 'No logs available.';
      return;
    }
    viewer.innerHTML = result.data.map(l =>
      `<span style="${l.level === 'error' ? 'color:var(--color-error);' : ''}">[${l.ts || ''}] ${l.msg}</span>`
    ).join('\n');
  }
}

customElements.define('agent-detail-page', AgentDetailPage);
