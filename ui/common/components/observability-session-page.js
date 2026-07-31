/**
 * Observability session detail — three panes: chat history, trace/span tree,
 * span detail (info/attributes with input/output messages).
 *
 * @element observability-session-page
 * @note Data sources (see /api/docs):
 *       `window.fetchObservabilitySession(sessionId)` → GET /api/observability/session/{id}
 *       `window.fetchObservabilityTrace(traceId)`     → GET /api/observability/trace/{id}
 *       `window.fetchSpanDetail(traceId, spanId)`     → GET /api/observability/span/{trace_id}/{span_id}
 *       `window.fetchChatSession(sessionId)`          → GET /api/chat/sessions/{id} (chat transcript)
 */
import styles from './observability-session-page.css' with { type: 'css' };
import { icons } from '../utils/icons.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class ObservabilitySessionPage extends HTMLElement {
  #initialized = false;
  #sessionId = '';
  #session = null;
  #spans = [];          // flattened {span, depth, traceId}
  #span = null;         // currently-selected span's detail payload
  #selected = null;     // {traceId, spanId}
  #detailTab = 'info';

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#sessionId = new URLSearchParams(window.location.search).get('session_id') || '';

    this.innerHTML = `
      <div class="page-head">
        <button class="back-btn" id="back-btn" type="button" aria-label="Back">${icons.arrowLeft('', 16)}</button>
        <h1 class="page-title">${this.#esc(this.#sessionId)}</h1>
      </div>
      <div class="kpi-strip" id="kpi-strip"></div>
      <div class="panes">
        <section class="pane" id="chat-pane" aria-label="Chat history">
          <h2 class="pane-title">Chat History</h2>
          <div class="pane-empty">Loading…</div>
        </section>
        <section class="pane" id="traces-pane" aria-label="Traces">
          <h2 class="pane-title">Traces</h2>
          <div class="pane-empty">Loading…</div>
        </section>
        <section class="pane" id="detail-pane" aria-label="Span detail">
          <div class="pane-empty">Select a span to see its details</div>
        </section>
      </div>
    `;

    this.querySelector('#back-btn').addEventListener('click', () => {
      window.location.href = '/observability.html';
    });
    this.querySelector('#traces-pane').addEventListener('click', (e) => {
      const row = e.target.closest('.span-row');
      if (row) this.#selectSpan(row.dataset.traceId, row.dataset.spanId);
    });
    this.querySelector('#detail-pane').addEventListener('click', (e) => {
      const tab = e.target.closest('.tab-btn');
      if (tab) {
        this.#detailTab = tab.dataset.tab;
        this.#renderDetailTabs();
      }
    });

    this.#load();
  }

  async #load() {
    await Promise.all([this.#loadSession(), this.#loadChat()]);
  }

  async #loadSession() {
    let resp;
    try {
      resp = await window.fetchObservabilitySession(this.#sessionId);
    } catch (e) {
      console.error('Session fetch failed:', e);
      this.querySelector('#traces-pane').innerHTML =
        '<h2 class="pane-title">Traces</h2><div class="pane-empty">Failed to load traces</div>';
      return;
    }
    this.#session = resp?.data?.session ?? null;
    this.#renderKpis();
    await this.#loadTraces();
  }

  #renderKpis() {
    const s = this.#session;
    if (!s) return;
    const kpi = (label, value) => `
      <div class="kpi">
        <div class="kpi-label">${label}</div>
        <div class="kpi-value">${value}</div>
      </div>`;
    this.querySelector('#kpi-strip').innerHTML = [
      kpi('Traces count', s.num_traces ?? 0),
      kpi('Total tokens', (s.token_usage?.total ?? 0).toLocaleString()),
      kpi('Total cost', `$ ${(s.cost_summary?.total?.cost ?? 0).toFixed(3)}`),
      kpi('Latency P50', `${((s.latency_p50 ?? 0) / 1000).toFixed(1)} s`),
      kpi('Latency P99', `${((s.latency_p99 ?? 0) / 1000).toFixed(1)} s`),
    ].join('');
  }

  /** Expand each trace of the session into a flattened, indented span list. */
  async #loadTraces() {
    const traces = this.#session?.traces ?? [];
    const flat = [];
    for (const entry of traces) {
      const traceId = entry.trace_id;
      let detail;
      try {
        detail = await window.fetchObservabilityTrace(traceId);
      } catch (e) {
        console.warn(`Trace ${traceId} fetch failed:`, e);
        continue;
      }
      const roots = detail?.spans ?? [];
      const walk = (node, depth) => {
        flat.push({ node, depth, traceId });
        (node.children || []).forEach((c) => walk(c, depth + 1));
      };
      roots.forEach((r) => walk(r, 0));
    }
    this.#spans = flat;
    this.#renderTraces();
    if (flat.length) this.#selectSpan(flat[0].traceId, flat[0].node.span_id);
  }

  #renderTraces() {
    const pane = this.querySelector('#traces-pane');
    if (!this.#spans.length) {
      pane.innerHTML = '<h2 class="pane-title">Traces</h2><div class="pane-empty">No trace data for this session</div>';
      return;
    }
    pane.innerHTML = `
      <h2 class="pane-title">Traces</h2>
      ${this.#spans.map(({ node, depth, traceId }) => `
        <button class="span-row" type="button"
          data-trace-id="${this.#esc(traceId)}" data-span-id="${this.#esc(node.span_id)}"
          style="margin-left:${depth * 16}px; width:calc(100% - ${depth * 16}px)">
          <span class="span-icon">${this.#spanIcon(node)}</span>
          <span class="span-name">${this.#esc(node.name)}</span>
          <span class="status-dot${this.#isError(node.status_code) ? ' is-error' : ''}"></span>
          <span class="chip">${icons.clock('', 12)} ${this.#fmtLatency(node.latency_ms)}</span>
        </button>
      `).join('')}
    `;
    this.#markSelected();
  }

  #markSelected() {
    this.querySelectorAll('.span-row').forEach((row) => {
      row.classList.toggle('is-selected',
        row.dataset.spanId === this.#selected?.spanId && row.dataset.traceId === this.#selected?.traceId);
    });
  }

  async #selectSpan(traceId, spanId) {
    this.#selected = { traceId, spanId };
    this.#markSelected();
    const pane = this.querySelector('#detail-pane');
    pane.innerHTML = '<div class="pane-empty">Loading span…</div>';
    let resp;
    try {
      resp = await window.fetchSpanDetail(traceId, spanId);
    } catch (e) {
      console.error('Span fetch failed:', e);
      pane.innerHTML = '<div class="pane-empty">Failed to load span details</div>';
      return;
    }
    this.#span = resp?.data?.span ?? null;
    this.#detailTab = 'info';
    this.#renderDetail();
  }

  #renderDetail() {
    const s = this.#span;
    const pane = this.querySelector('#detail-pane');
    if (!s) {
      pane.innerHTML = '<div class="pane-empty">Select a span to see its details</div>';
      return;
    }
    const tokens = s.token_count_total ?? 0;
    const cost = s.cost_summary?.total?.cost ?? 0;
    pane.innerHTML = `
      <div class="detail-head">
        <h3>${this.#esc(s.name)}</h3>
        <span class="badge-kind">${this.#esc(s.span_kind || 'internal')}</span>
      </div>
      <div class="detail-chips">
        <span class="chip">${icons.clock('', 12)} ${s.latency_ms != null ? `${Math.round(s.latency_ms)}ms` : '—'}</span>
        <span class="chip">${icons.layers('', 12)} ${tokens.toLocaleString()}</span>
        <span class="chip">${icons.code('', 12)} $${cost.toFixed(3)}</span>
        <span class="chip">${icons.calendar('', 12)} ${this.#fmtDate(s.start_time)}</span>
      </div>
      <div class="tabs" role="tablist">
        <button class="tab-btn${this.#detailTab === 'info' ? ' is-active' : ''}" data-tab="info" type="button">${icons.info('', 14)} Info</button>
        <button class="tab-btn${this.#detailTab === 'attributes' ? ' is-active' : ''}" data-tab="attributes" type="button">${icons.document('', 14)} Attributes</button>
      </div>
      <div id="detail-body"></div>
    `;
    this.#renderDetailTabs();
  }

  #renderDetailTabs() {
    this.querySelectorAll('.tab-btn').forEach((b) =>
      b.classList.toggle('is-active', b.dataset.tab === this.#detailTab));
    const body = this.querySelector('#detail-body');
    if (!body) return;
    body.innerHTML = this.#detailTab === 'info' ? this.#infoTabHtml() : this.#attributesTabHtml();
  }

  #infoTabHtml() {
    const s = this.#span;
    const inputMsgs = this.#extractMessages(s.attributes?.llm?.input_messages, s.input?.value);
    const outputMsgs = this.#extractMessages(s.attributes?.llm?.output_messages, s.output?.value);
    const section = (title, msgs, emptyText) => `
      <div class="detail-section-title">${title}</div>
      ${msgs.length
        ? msgs.map((m) => `
            <div class="msg-block">
              <div class="msg-role">${this.#esc(m.role || '')}</div>
              <div class="msg-content">${this.#esc(m.content || '')}</div>
            </div>`).join('')
        : `<div class="pane-empty">${emptyText}</div>`}
    `;
    return `
      ${section('Input messages', inputMsgs, 'No input message available')}
      ${section('Output messages', outputMsgs, 'No output message available')}
    `;
  }

  #attributesTabHtml() {
    const attrs = this.#span?.attributes ?? {};
    return `<pre class="raw-json">${this.#esc(JSON.stringify(attrs, null, 2))}</pre>`;
  }

  /** Messages may live in OTel genai attributes or in the raw input/output value. */
  #extractMessages(attrMsgs, rawValue) {
    if (Array.isArray(attrMsgs) && attrMsgs.length) {
      return attrMsgs.map((m) => {
        const msg = m.message || m;
        return { role: msg.role, content: typeof msg.content === 'string' ? msg.content : JSON.stringify(msg.content) };
      });
    }
    if (!rawValue) return [];
    try {
      const parsed = typeof rawValue === 'string' ? JSON.parse(rawValue) : rawValue;
      if (Array.isArray(parsed?.messages)) {
        return parsed.messages.map((m) => ({
          role: m.role,
          content: typeof m.content === 'string' ? m.content : JSON.stringify(m.content),
        }));
      }
    } catch { /* plain text below */ }
    return [{ role: '', content: String(rawValue) }];
  }

  async #loadChat() {
    const pane = this.querySelector('#chat-pane');
    let messages = [];
    try {
      const resp = await window.fetchChatSession(this.#sessionId);
      messages = resp?.data ?? [];
    } catch {
      // Observability sessions don't always map to a chat session.
    }
    if (!messages.length) {
      pane.innerHTML = '<h2 class="pane-title">Chat History</h2><div class="pane-empty">No chat transcript for this session</div>';
      return;
    }
    const s = this.#session;
    pane.innerHTML = `
      <h2 class="pane-title">Chat History</h2>
      <div class="chat-card">
        ${messages.map((m) => m.role === 'user'
          ? `<div class="msg-user">${this.#esc(m.content)}</div>`
          : `<div class="msg-assistant">${this.#esc(m.content)}</div>`).join('')}
        <div class="chat-meta">
          <span class="chip">${icons.layers('', 12)} ${(s?.token_usage?.total ?? 0).toLocaleString()}</span>
          <span class="chip">$ ${(s?.cost_summary?.total?.cost ?? 0).toFixed(2)}</span>
          <span class="chip">${icons.clock('', 12)} ${((s?.latency_p50 ?? 0) / 1000).toFixed(1)} s</span>
        </div>
      </div>
    `;
  }

  #spanIcon(node) {
    if (node.name?.toLowerCase().includes('chatcompletion') || node.model) return icons.cube('', 14);
    if (node.name?.toLowerCase().startsWith('tool')) return icons.terminal('', 14);
    return icons.trace('', 14);
  }

  #isError(status) {
    return typeof status === 'string' && status.toUpperCase().includes('ERROR');
  }

  #fmtLatency(ms) {
    if (ms == null) return '—';
    return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${Math.round(ms)}ms`;
  }

  #fmtDate(iso) {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '—';
    return `${d.toLocaleDateString('en-US')}, ${d.toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit' })}`;
  }

  #esc(str) {
    if (str == null) return '';
    return String(str).replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('observability-session-page', ObservabilitySessionPage);
