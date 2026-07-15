import { icons } from '/common/utils/icons.js';

import styles from './session-trace-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

/**
 * Renders GET /api/observability/trace/{trace_id} (see `nasiko observe trace`).
 * Response envelope: { data: { trace: TraceDetail } } where TraceDetail carries
 * num_spans / latency_ms / cost_summary and a nested `spans` tree — children are
 * embedded in each SpanNode, not repeated in the top-level slice.
 */
class SessionTracePage extends HTMLElement {
  connectedCallback() {
    const traceId = new URLSearchParams(location.search).get('trace_id')
      || this.getAttribute('trace-id');
    if (!traceId) {
      this.innerHTML = `<div class="empty-state">No trace ID specified.</div>`;
      return;
    }
    document.title = `Nasiko — Trace ${traceId.slice(0, 12)}`;
    this.innerHTML = `
      <div class="trace-header">
        <a class="back-link" href="javascript:history.back()">${icons.chevronLeft('', 16)} Back</a>
        <h1>${traceId}</h1>
      </div>
      <app-skeleton height="200px"></app-skeleton>
    `;
    this.#load(traceId);
  }

  async #load(traceId) {
    let trace;
    try {
      trace = await window.fetchTraceDetail(traceId);
    } catch {
      this.querySelector('app-skeleton')?.remove();
      this.insertAdjacentHTML('beforeend', `
        <div class="empty-state">
          <p>Trace not yet available.</p>
          <p style="font-size:var(--font-size-xs);margin-top:var(--space-sm)">
            Traces may take a few seconds to appear in Tempo after the request completes.
            Try refreshing in a moment.
          </p>
        </div>
      `);
      return;
    }
    if (!trace || !Array.isArray(trace.spans)) {
      this.querySelector('app-skeleton')?.remove();
      this.insertAdjacentHTML('beforeend', `<div class="empty-state">Trace not found or observability backend unavailable.</div>`);
      return;
    }

    const usage = this.#computeUsage(trace.spans);
    const cost = trace.cost_summary?.total?.cost;

    this.querySelector('app-skeleton')?.remove();

    const statsHtml = `
      <div class="stats-row">
        <div class="stat">
          <span class="stat-label">Spans</span>
          <span class="stat-value">${trace.num_spans ?? this.#countSpans(trace.spans)}</span>
        </div>
        <div class="stat">
          <span class="stat-label">Duration</span>
          <span class="stat-value">${this.#fmtDuration(trace.latency_ms)}</span>
        </div>
        <div class="stat">
          <span class="stat-label">Tokens</span>
          <span class="stat-value">${usage.total.toLocaleString()}</span>
        </div>
        <div class="stat">
          <span class="stat-label">Cost</span>
          <span class="stat-value">${cost != null ? `$${this.#fmtCost(cost)}` : '—'}</span>
        </div>
        ${trace.project_session_id ? `
        <div class="stat">
          <span class="stat-label">Session</span>
          <span class="stat-value stat-value--id">${this.#esc(trace.project_session_id)}</span>
        </div>` : ''}
      </div>
    `;

    const spansHtml = trace.spans.length
      ? `<div class="spans-list">${trace.spans.map(s => this.#renderSpanNode(s, 0)).join('')}</div>`
      : `<div class="empty-state">No spans in this trace.</div>`;

    this.insertAdjacentHTML('beforeend', statsHtml + spansHtml);

    this.querySelectorAll('.span-item').forEach(el => {
      el.addEventListener('click', () => {
        const detail = el.nextElementSibling;
        if (detail?.classList.contains('span-detail')) {
          detail.classList.toggle('is-visible');
          el.classList.toggle('is-expanded');
        }
      });
    });
  }

  #renderSpanNode(s, depth) {
    const isError = s.status_code === 'ERROR';
    const tokens = (s.input_tokens || 0) + (s.output_tokens || 0);
    const indent = depth > 0 ? `<span class="span-indent" style="margin-left:${(depth - 1) * 16}px"></span>` : '';

    const detailRows = [
      ['kind', s.span_kind],
      ['span_id', s.span_id],
      ['status', s.status_code],
      ['started', s.start_time],
      ['ended', s.end_time],
      ['model', s.model],
      ['tokens in / out', tokens ? `${s.input_tokens} / ${s.output_tokens}` : null],
    ].filter(([, v]) => v != null && v !== '');

    let html = `
      <div class="span-item">
        <div class="span-name">
          ${indent}
          <span class="span-dot${isError ? ' is-error' : ''}"></span>
          ${this.#esc(s.name)}
          ${s.model ? `<span style="opacity:0.5;font-size:11px">${this.#esc(s.model)}</span>` : ''}
        </div>
        <div class="span-meta">
          ${tokens ? `<span>${tokens.toLocaleString()} tok</span>` : ''}
          <span>${this.#fmtDuration(s.latency_ms)}</span>
        </div>
      </div>
      <div class="span-detail">
        <div class="span-attrs">
          ${detailRows.map(([k, v]) => `<span class="attr-key">${this.#esc(k)}</span><span class="attr-val">${this.#esc(String(v))}</span>`).join('')}
        </div>
      </div>
    `;

    for (const child of s.children || []) {
      html += this.#renderSpanNode(child, depth + 1);
    }
    return html;
  }

  #computeUsage(spans) {
    let input = 0, output = 0;
    const walk = (nodes) => {
      for (const s of nodes) {
        input += Number(s.input_tokens || 0);
        output += Number(s.output_tokens || 0);
        walk(s.children || []);
      }
    };
    walk(spans);
    return { input, output, total: input + output };
  }

  #countSpans(spans) {
    let n = 0;
    const walk = (nodes) => {
      for (const s of nodes) {
        n += 1;
        walk(s.children || []);
      }
    };
    walk(spans);
    return n;
  }

  #fmtCost(cost) {
    return cost < 0.01 ? cost.toFixed(4) : cost.toFixed(3);
  }

  #fmtDuration(ms) {
    if (ms == null || ms === 0) return '—';
    if (ms < 1000) return `${Math.round(ms)}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('session-trace-page', SessionTracePage);
