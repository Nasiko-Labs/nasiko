/**
 * Observability — execution history. Lists every session across agents;
 * selecting one opens its trace details (observability-session.html).
 *
 * @element observability-page
 * @note Data source: `window.fetchObservabilitySessions()` →
 *       GET /api/observability/session/list (see /api/docs), which returns
 *       `{ data: { sessions: [SessionSummary], … } }`.
 */
import styles from './observability-page.css' with { type: 'css' };
import { icons } from '../utils/icons.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class ObservabilityPage extends HTMLElement {
  #initialized = false;
  #sessions = [];
  #query = '';

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <h1 class="page-title">Execution history</h1>
      <p class="page-sub">Review all queries across agents. Select a session to open its query trace details.</p>
      <div class="search-wrap">
        ${icons.search('', 16)}
        <input type="search" id="session-search" placeholder="Search sessions by ID or agent" />
      </div>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Sessions</th>
              <th>Traces count</th>
              <th>Tokens</th>
              <th>Latency P50</th>
              <th>Date</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody id="sessions-tbody">
            <tr class="empty-row"><td colspan="6">Loading…</td></tr>
          </tbody>
        </table>
      </div>
    `;

    this.querySelector('#session-search').addEventListener('input', (e) => {
      this.#query = e.target.value.trim().toLowerCase();
      this.#renderRows();
    });
    this.querySelector('#sessions-tbody').addEventListener('click', (e) => {
      const row = e.target.closest('tr[data-session-id]');
      if (row) window.location.href = `/observability-session.html?session_id=${encodeURIComponent(row.dataset.sessionId)}`;
    });

    this.#load();
  }

  async #load() {
    let resp;
    try {
      resp = await window.fetchObservabilitySessions();
    } catch (e) {
      console.error('Session list fetch failed:', e);
      this.querySelector('#sessions-tbody').innerHTML =
        '<tr class="empty-row"><td colspan="6">Failed to load sessions</td></tr>';
      return;
    }
    this.#sessions = resp?.data?.sessions ?? [];
    this.#renderRows();
  }

  #renderRows() {
    const tbody = this.querySelector('#sessions-tbody');
    const rows = this.#query
      ? this.#sessions.filter((s) =>
          (s.session_id || '').toLowerCase().includes(this.#query) ||
          (s.agent_id || '').toLowerCase().includes(this.#query))
      : this.#sessions;
    if (!rows.length) {
      tbody.innerHTML = '<tr class="empty-row"><td colspan="6">No sessions yet</td></tr>';
      return;
    }
    tbody.innerHTML = rows.map((s) => `
      <tr data-session-id="${this.#esc(s.session_id)}">
        <td class="session-id" title="${this.#esc(s.session_id)}">${this.#esc(this.#shorten(s.session_id))}</td>
        <td>${s.num_traces ?? 0}</td>
        <td>${(s.token_usage?.total ?? 0).toLocaleString()}</td>
        <td>${this.#fmtLatency(s.trace_latency_ms_p50)}</td>
        <td>${this.#fmtDate(s.start_time)}</td>
        <td>
          <button class="traces-btn" type="button">Traces ${icons.chevronRight('', 14)}</button>
        </td>
      </tr>
    `).join('');
  }

  #shorten(id) {
    return id && id.length > 28 ? `${id.slice(0, 25)}…` : id || '—';
  }

  #fmtLatency(ms) {
    return ms == null ? '—' : (ms / 1000).toFixed(2);
  }

  #fmtDate(iso) {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '—';
    const date = d.toLocaleDateString('en-GB', { day: '2-digit', month: 'short', year: 'numeric' });
    const time = d.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' });
    return `${date} at ${time}`;
  }

  #esc(str) {
    if (!str) return '';
    return String(str).replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('observability-page', ObservabilityPage);
