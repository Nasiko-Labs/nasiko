const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (sessions-page) {
  :scope {
    display: block;
    max-width: 720px;
    margin: 0 auto;
    padding: var(--space-lg) var(--space-md);
  }
  .sessions-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-lg);
  }
  .sessions-header h1 {
    font-size: var(--font-size-xl);
    font-weight: 600;
  }
  .session-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-xs);
  }
  .session-card {
    display: flex;
    align-items: center;
    gap: var(--space-md);
    padding: var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    text-decoration: none;
    color: inherit;
    transition: border-color 0.15s;
  }
  .session-card:hover { border-color: var(--color-primary); }
  .session-info { flex: 1; min-width: 0; }
  .session-agent {
    font-size: var(--font-size-sm);
    font-weight: 500;
    color: var(--color-text-main);
  }
  .session-preview {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    margin-top: 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .session-time {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    flex-shrink: 0;
  }
  .empty {
    text-align: center;
    padding: var(--space-xl);
    color: var(--color-text-muted);
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class SessionsPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <div class="sessions-header">
        <h1>Sessions</h1>
        <app-button variant="primary" size="sm" id="btn-new">New Chat</app-button>
      </div>
      <div class="session-list" id="session-list">
        <app-skeleton height="60px"></app-skeleton>
        <app-skeleton height="60px"></app-skeleton>
        <app-skeleton height="60px"></app-skeleton>
      </div>
    `;

    this.querySelector('#btn-new')?.addEventListener('click', () => {
      window.location.href = '/chat.html';
    });

    this.#load();
  }

  async #load() {
    const list = this.querySelector('#session-list');
    try {
      const result = await window.fetchSessions('', 1, 50);
      const sessions = result?.data || [];

      if (!sessions.length) {
        list.innerHTML = '<div class="empty">No chat sessions yet. Start one from the orchestrator or an agent page.</div>';
        return;
      }

      list.innerHTML = sessions.map(s => {
        const agentName = s.agent_name || 'Orchestrator';
        const preview = s.last_message || '';
        const time = s.updated_at || s.created_at;
        const timeStr = time ? this.#relativeTime(new Date(time)) : '';
        const sessionId = s.session_id || s.id;
        const href = `/chat.html?session_id=${encodeURIComponent(sessionId)}&agent_id=${encodeURIComponent(s.agent_id || '')}&agent_name=${encodeURIComponent(agentName)}`;

        return `<a class="session-card" href="${href}">
          <div class="session-info">
            <div class="session-agent">${this.#esc(agentName)}</div>
            ${preview ? `<div class="session-preview">${this.#esc(preview.slice(0, 100))}</div>` : ''}
          </div>
          <div class="session-time">${timeStr}</div>
        </a>`;
      }).join('');
    } catch {
      list.innerHTML = '<div class="empty">Failed to load sessions.</div>';
    }
  }

  #relativeTime(date) {
    const now = new Date();
    const diff = Math.floor((now - date) / 1000);
    if (diff < 60) return 'just now';
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
    return date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('sessions-page', SessionsPage);
