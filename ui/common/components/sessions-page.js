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
        <div class="sessions-header-info">
          <h1 class="title-page">Execution history</h1>
          <p class="sessions-subtitle">Review all queries across agents. Select a session to open its trace details.</p>
        </div>
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
    const row = `<div class="session-row-skeleton">
      <div class="skeleton-info">
        <div class="skeleton-line skeleton-line--short"></div>
        <div class="skeleton-line skeleton-line--long"></div>
      </div>
      <div class="skeleton-line skeleton-line--time"></div>
    </div>`;
    return `<div class="sessions-table-skeleton">
      <div class="skeleton-line skeleton-line--head"></div>
      ${row.repeat(4)}
    </div>`;
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

    const rows = sessions.map(s => this.#renderRow(s)).join('');
    list.innerHTML = `
      <table class="sessions-table">
        <thead>
          <tr>
            <th>Sessions</th>
            <th>Traces count</th>
            <th>Tokens</th>
            <th>Latency P50</th>
            <th>Date</th>
            <th class="col-actions">Actions</th>
          </tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    `;

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

    list.querySelectorAll('tr[data-href]').forEach(row => {
      row.addEventListener('click', () => { window.location.href = row.dataset.href; });
    });
  }

  #renderRow(s) {
    const agentName = s.agent_name || 'Orchestrator';
    const preview = s.last_message || '';
    const time = s.updated_at || s.created_at;
    const timeStr = time ? this.#formatDate(new Date(time)) : '—';
    const sessionId = s.session_id;
    const href = `/chat.html?session_id=${encodeURIComponent(sessionId)}&agent_id=${encodeURIComponent(s.agent_id || '')}&agent_name=${encodeURIComponent(agentName)}`;
    const msgCount = s.message_count ? `<span class="session-msg-count">${s.message_count} msgs</span>` : '';
    const o = this.#obsStats.get(sessionId);
    const traces = o?.num_traces ?? '—';
    const tokens = o?.token_usage?.total ? this.#fmtCount(o.token_usage.total) : '—';
    const p50 = o?.trace_latency_ms_p50 ? this.#fmtMs(o.trace_latency_ms_p50) : '—';

    return `<tr data-href="${href}" tabindex="0">
      <td class="col-session">
        <div class="session-agent">${this.#esc(agentName)}${msgCount}</div>
        ${preview ? `<div class="session-preview">${this.#esc(preview.slice(0, 90))}</div>` : ''}
      </td>
      <td class="col-num">${traces}</td>
      <td class="col-num">${tokens}</td>
      <td class="col-num">${p50}</td>
      <td class="col-date">${timeStr}</td>
      <td class="col-actions">
        <button class="session-traces" type="button" data-session-id="${this.#esc(sessionId)}"
          title="View traces" aria-label="View traces for this session"><span>Traces</span>${icons.chevronRight('', 14)}</button>
        <button class="session-delete" data-session-id="${this.#esc(sessionId)}" title="Delete session" aria-label="Delete session">${icons.trash('', 14)}</button>
      </td>
    </tr>`;
  }

  #formatDate(date) {
    const now = new Date();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const yesterday = new Date(today);
    yesterday.setDate(yesterday.getDate() - 1);
    const timeStr = date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
    if (date >= today) return `Today at ${timeStr}`;
    if (date >= yesterday) return `Yesterday at ${timeStr}`;
    const dateStr = date.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
    return `${dateStr} at ${timeStr}`;
  }

  #fmtCount(n) {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return String(n);
  }

  #fmtMs(ms) {
    return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`;
  }

  async #deleteSession(sessionId) {
    let card = null;
    for (const btn of this.querySelectorAll('.session-delete')) {
      if (btn.dataset.sessionId === sessionId) {
        card = btn.closest('tr');
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

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('sessions-page', SessionsPage);
