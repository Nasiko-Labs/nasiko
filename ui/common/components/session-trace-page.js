import { icons } from '/common/utils/icons.js';

import styles from './session-trace-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

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
    let data;
    try {
      data = await window.fetchTraceDetail(traceId);
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
    if (!data || !data.spans) {
      this.querySelector('app-skeleton')?.remove();
      this.insertAdjacentHTML('beforeend', `<div class="empty-state">Trace not found or observability backend unavailable.</div>`);
      return;
    }

    const spans = data.spans || [];
    const usage = this.#computeUsage(spans);
    const durationMs = data.duration_ms || 0;

    this.querySelector('app-skeleton')?.remove();

    const statsHtml = `
      <div class="stats-row">
        <div class="stat">
          <span class="stat-label">Spans</span>
          <span class="stat-value">${spans.length}</span>
        </div>
        <div class="stat">
          <span class="stat-label">Duration</span>
          <span class="stat-value">${this.#fmtDuration(durationMs)}</span>
        </div>
        <div class="stat">
          <span class="stat-label">Tokens</span>
          <span class="stat-value">${usage.total.toLocaleString()}</span>
        </div>
        <div class="stat">
          <span class="stat-label">Est. cost</span>
          <span class="stat-value">$${this.#estimateCost(usage)}</span>
        </div>
      </div>
    `;

    const tree = this.#buildTree(spans);
    const spansHtml = tree.length
      ? `<div class="spans-list">${tree.map(n => this.#renderSpanNode(n, 0)).join('')}</div>`
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

  #buildTree(spans) {
    const map = new Map();
    const roots = [];
    for (const s of spans) {
      map.set(s.span_id, { span: s, children: [] });
    }
    for (const s of spans) {
      const node = map.get(s.span_id);
      if (s.parent_span_id && map.has(s.parent_span_id)) {
        map.get(s.parent_span_id).children.push(node);
      } else {
        roots.push(node);
      }
    }
    roots.sort((a, b) => new Date(a.span.started_at) - new Date(b.span.started_at));
    return roots;
  }

  #renderSpanNode(node, depth) {
    const s = node.span;
    const isError = s.attributes?.['otel.status_code'] === 'ERROR';
    const model = s.attributes?.['gen_ai.request.model'] || '';
    const inputTokens = s.attributes?.['gen_ai.usage.input_tokens'];
    const outputTokens = s.attributes?.['gen_ai.usage.output_tokens'];
    const indent = depth > 0 ? `<span class="span-indent" style="margin-left:${(depth - 1) * 16}px"></span>` : '';

    const attrs = s.attributes || {};
    const attrEntries = Object.entries(attrs)
      .filter(([k]) => !k.startsWith('__'))
      .map(([k, v]) => `<span class="attr-key">${this.#esc(k)}</span><span class="attr-val">${this.#esc(String(v))}</span>`)
      .join('');

    const tokenBadge = inputTokens != null
      ? `<span>${Number(inputTokens) + Number(outputTokens || 0)} tok</span>`
      : '';

    let html = `
      <div class="span-item">
        <div class="span-name">
          ${indent}
          <span class="span-dot${isError ? ' is-error' : ''}"></span>
          ${this.#esc(s.name)}
          ${model ? `<span style="opacity:0.5;font-size:11px">${this.#esc(model)}</span>` : ''}
        </div>
        <div class="span-meta">
          ${tokenBadge}
          <span>${this.#fmtDuration(s.duration_ms || 0)}</span>
        </div>
      </div>
      <div class="span-detail">
        <div class="span-attrs">
          <span class="attr-key">service</span><span class="attr-val">${this.#esc(s.service_name)}</span>
          <span class="attr-key">span_id</span><span class="attr-val">${s.span_id}</span>
          ${attrEntries}
        </div>
      </div>
    `;

    for (const child of node.children) {
      html += this.#renderSpanNode(child, depth + 1);
    }
    return html;
  }

  #computeUsage(spans) {
    let input = 0, output = 0;
    for (const s of spans) {
      input += Number(s.attributes?.['gen_ai.usage.input_tokens'] || 0);
      output += Number(s.attributes?.['gen_ai.usage.output_tokens'] || 0);
    }
    return { input, output, total: input + output };
  }

  #estimateCost(usage) {
    const cost = (usage.input * 0.000003) + (usage.output * 0.000015);
    return cost < 0.01 ? cost.toFixed(4) : cost.toFixed(3);
  }

  #fmtDuration(ms) {
    if (ms == null || ms === 0) return '—';
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('session-trace-page', SessionTracePage);
