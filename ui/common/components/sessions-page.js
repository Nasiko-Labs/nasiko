import styles from './sessions-page.css' with { type: 'css' };
import { icons } from '../utils/icons.js';
import './app-button.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class SessionsPage extends HTMLElement {
  #initialized = false;
  #sessions = [];
  #obsStats = new Map();

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#render();
    this.#load();
  }

  #render() {
    this.innerHTML = `
      <div class="sessions-header">
        <h1>Sessions</h1>
        <app-button variant="dark" size="sm" id="btn-new">New Chat</app-button>
      </div>
      <div class="sessions-search-wrap">
        <span class="sessions-search-icon">${icons.search('', 16)}</span>
        <input type="search" class="sessions-search" placeholder="Filter sessions..." aria-label="Filter sessions" />
      </div>
      <div class="session-list" id="session-list">
        ${this.#renderSkeletons()}
      </div>
    `;

    this.querySelector('#btn-new')?.addEventListener('click', () => {
      window.location.href = '/chat.html';
    });

    this.querySelector('.sessions-search')?.addEventListener('input', (e) => {
      this.#filterSessions(e.target.value);
    });
  }

  #renderSkeletons() {
    const skeleton = `<div class="session-card-skeleton">
      <div class="skeleton-avatar"></div>
      <div class="skeleton-info">
        <div class="skeleton-line skeleton-line--short"></div>
        <div class="skeleton-line skeleton-line--long"></div>
      </div>
      <div class="skeleton-line skeleton-line--time"></div>
    </div>`;
    return skeleton.repeat(4);
  }

  async #load() {
    const list = this.querySelector('#session-list');
    try {
      // Chat sessions are the primary source; observability stats (traces,
      // tokens, latency) are joined in by session id — best-effort, the page
      // works without them.
      const [chatRes, obsRes] = await Promise.allSettled([
        window.fetchSessions('', 1, 50),
        window.fetchObservabilitySessions?.() ?? Promise.reject(),
      ]);
      if (chatRes.status === 'rejected') throw chatRes.reason;
      const sessions = chatRes.value?.data || [];

      this.#obsStats = new Map();
      if (obsRes.status === 'fulfilled') {
        const obsSessions = obsRes.value?.data?.sessions || obsRes.value?.sessions || [];
        for (const o of obsSessions) {
          const id = o.session_id || o.id;
          if (id) this.#obsStats.set(id, o);
        }
      }

      this.#sessions = sessions;
      this.#renderSessions(sessions);
    } catch {
      list.innerHTML = `<app-empty-state
        title="Failed to load sessions"
        description="Something went wrong while loading your chat sessions."
        icon='${icons.xCircle()}'>
        <app-button variant="secondary" size="sm" id="btn-retry">Retry</app-button>
      </app-empty-state>`;
      this.querySelector('#btn-retry')?.addEventListener('click', () => {
        list.innerHTML = this.#renderSkeletons();
        this.#load();
      });
    }
  }

  #filterSessions(query) {
    const q = (query || '').toLowerCase().trim();
    if (!q) {
      this.#renderSessions(this.#sessions);
      return;
    }
    const filtered = this.#sessions.filter(s => {
      const agent = (s.agent_name || 'Orchestrator').toLowerCase();
      const msg = (s.last_message || '').toLowerCase();
      return agent.includes(q) || msg.includes(q);
    });
    this.#renderSessions(filtered);
  }

  #renderSessions(sessions) {
    const list = this.querySelector('#session-list');
    if (!sessions.length) {
      if (this.#sessions.length > 0) {
        list.innerHTML = `<app-empty-state
          title="No matching sessions"
          description="Try a different search term."
          icon='${icons.search()}'>
        </app-empty-state>`;
      } else {
        list.innerHTML = `<app-empty-state
          title="No sessions yet"
          description="Start a conversation from the orchestrator or an agent page."
          icon='${icons.send()}'>
          <app-button variant="dark" size="sm" onclick="window.location.href='/index.html'">Start a Chat</app-button>
        </app-empty-state>`;
      }
      return;
    }

    const groups = this.#groupByDate(sessions);
    let html = '';
    for (const [label, items] of groups) {
      html += `<div class="session-group-header">${this.#esc(label)}</div>`;
      html += items.map(s => this.#renderCard(s)).join('');
    }
    list.innerHTML = html;

    list.querySelectorAll('.session-delete').forEach(btn => {
      btn.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        const sessionId = btn.dataset.sessionId;
        this.#deleteSession(sessionId);
      });
    });

    list.querySelectorAll('.session-traces').forEach(btn => {
      btn.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        window.location.href = `/observability-session.html?session_id=${encodeURIComponent(btn.dataset.sessionId)}`;
      });
    });
  }

  #renderCard(s) {
    const agentName = s.agent_name || 'Orchestrator';
    const preview = s.last_message || '';
    const time = s.updated_at || s.created_at;
    const timeStr = time ? this.#relativeTime(new Date(time)) : '';
    const sessionId = s.session_id;
    const href = `/chat.html?session_id=${encodeURIComponent(sessionId)}&agent_id=${encodeURIComponent(s.agent_id || '')}&agent_name=${encodeURIComponent(agentName)}`;
    const initial = agentName.charAt(0).toUpperCase();
    const msgCount = s.message_count ? `<span class="session-msg-count">${s.message_count} msgs</span>` : '';
    const stats = this.#statsChips(this.#obsStats.get(sessionId));

    return `<a class="session-card" href="${href}">
      <div class="session-avatar">${initial}</div>
      <div class="session-info">
        <div class="session-agent">${this.#esc(agentName)}${msgCount}</div>
        ${preview ? `<div class="session-preview">${this.#esc(preview.slice(0, 120))}</div>` : ''}
        ${stats}
      </div>
      <div class="session-meta">
        <span class="session-time">${timeStr}</span>
        <button class="session-traces" type="button" data-session-id="${this.#esc(sessionId)}"
          title="View traces" aria-label="View traces for this session">${icons.trace('', 14)}<span>Traces</span></button>
        <button class="session-delete" data-session-id="${this.#esc(sessionId)}" title="Delete session" aria-label="Delete session">${icons.trash('', 14)}</button>
      </div>
    </a>`;
  }

  /** Observability chips for a session: traces, tokens, p50 latency. */
  #statsChips(o) {
    if (!o) return '';
    const chips = [];
    const traces = o.num_traces;
    if (traces) chips.push(`${icons.workflow('chip-icon', 12)}${traces} ${traces === 1 ? 'trace' : 'traces'}`);
    const tokens = o.token_usage?.total;
    if (tokens) chips.push(`${icons.coins('chip-icon', 12)}${this.#fmtCount(tokens)} tok`);
    const p50 = o.trace_latency_ms_p50;
    if (p50) chips.push(`${icons.clock('chip-icon', 12)}${this.#fmtMs(p50)} p50`);
    if (!chips.length) return '';
    return `<div class="session-stats">${chips.map((c) => `<span class="session-chip">${c}</span>`).join('')}</div>`;
  }

  #fmtCount(n) {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return String(n);
  }

  #fmtMs(ms) {
    return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`;
  }

  #groupByDate(sessions) {
    const now = new Date();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const yesterday = new Date(today);
    yesterday.setDate(yesterday.getDate() - 1);
    const weekAgo = new Date(today);
    weekAgo.setDate(weekAgo.getDate() - 7);

    const groups = new Map();
    for (const s of sessions) {
      const time = s.updated_at || s.created_at;
      const date = time ? new Date(time) : new Date(0);
      let label;
      if (date >= today) {
        label = 'Today';
      } else if (date >= yesterday) {
        label = 'Yesterday';
      } else if (date >= weekAgo) {
        label = 'This Week';
      } else {
        label = 'Older';
      }
      if (!groups.has(label)) groups.set(label, []);
      groups.get(label).push(s);
    }
    return groups;
  }

  async #deleteSession(sessionId) {
    let card = null;
    for (const btn of this.querySelectorAll('.session-delete')) {
      if (btn.dataset.sessionId === sessionId) {
        card = btn.closest('.session-card');
        break;
      }
    }
    if (card) card.style.opacity = '0.4';
    try {
      if (window.deleteSession) {
        await window.deleteSession(sessionId);
      }
      this.#sessions = this.#sessions.filter(s => s.session_id !== sessionId);
      const query = this.querySelector('.sessions-search')?.value || '';
      this.#filterSessions(query);
    } catch {
      if (card) card.style.opacity = '1';
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
