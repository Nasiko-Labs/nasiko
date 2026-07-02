import styles from './sessions-page.css' with { type: 'css' };
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
