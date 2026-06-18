/**
 * Simple SVG line chart for time-series or sequential numeric data.
 *
 * @element app-line-chart
 * @attr {string} title - Chart title
 * @attr {string} unit - Unit label for the Y-axis values
 * @attr {string} empty-text - Text shown when there is no data
 * @attr {string} period - Active period label shown in the header
 * @prop {Array} data - Set chart data: array of `{ label, value }` objects (JS setter)
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-line-chart) {
  :scope { display: block; }

  .chart-wrap {
    background: var(--color-bg-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md) var(--space-md) var(--space-sm);
    display: flex;
    flex-direction: column;
    gap: var(--space-xs);
  }

  .chart-hdr {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-sm);
    min-height: 1.4em;
  }

  .chart-title {
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--color-text-main);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chart-meta {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    flex-shrink: 0;
  }

  .chart-svg {
    display: block;
    width: 100%;
    height: 160px;
  }
}
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

let _seq = 0;

/**
 * <app-line-chart>
 *
 * Attributes:
 *   title       — chart heading
 *   unit        — y-axis unit shown in header (e.g. "threads", "req/s")
 *   empty-text  — message when dataFn returns no points
 *   period      — read-only hint for callers; changing it triggers refresh()
 *
 * Properties:
 *   dataFn      — async () => [{x: epochMs, y: number}, ...]
 *
 * Methods:
 *   setData(points)  — replace data and repaint immediately
 *   refresh()        — call dataFn, show skeleton while loading, then repaint
 */
export class AppLineChart extends HTMLElement {
  #id = ++_seq;
  #points = [];
  #loading = false;
  #error = null;
  #initialized = false;

  dataFn = null;

  static get observedAttributes() {
    return ['title', 'unit', 'empty-text', 'period'];
  }

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#render();
  }

  attributeChangedCallback(name, old, next) {
    if (old === next || !this.#initialized) return;
    if (name === 'title') {
      const el = this.querySelector('.chart-title');
      if (el) el.textContent = next ?? '';
    } else if (name === 'period') {
      this.refresh();
    }
  }

  setData(points) {
    this.#points = Array.isArray(points) ? points : [];
    this.#error = null;
    this.#loading = false;
    this.#updateMeta();
    this.#paint();
  }

  async refresh() {
    if (!this.dataFn) return;
    this.#loading = true;
    this.#updateMeta();
    this.#paintSkeleton();
    try {
      this.#points = (await this.dataFn()) ?? [];
      this.#error = null;
    } catch (e) {
      this.#error = e.message ?? 'Error';
      this.#points = [];
    } finally {
      this.#loading = false;
      this.#updateMeta();
      this.#paint();
    }
  }

  #render() {
    this.innerHTML = `
      <div class="chart-wrap">
        <div class="chart-hdr">
          <span class="chart-title">${this.#esc(this.getAttribute('title') ?? '')}</span>
          <span class="chart-meta"></span>
        </div>
        <svg class="chart-svg" viewBox="0 0 400 160" preserveAspectRatio="none" aria-hidden="true"></svg>
      </div>`;
    this.#paintSkeleton();
  }

  #updateMeta() {
    const el = this.querySelector('.chart-meta');
    if (!el) return;
    el.textContent = this.#loading ? 'Loading…' : (this.getAttribute('unit') || '');
  }

  #paintSkeleton() {
    const svg = this.querySelector('.chart-svg');
    if (!svg) return;
    svg.innerHTML = `
      <rect x="48" y="18" width="210" height="6" rx="3" fill="var(--color-border)" opacity=".7"/>
      <rect x="48" y="36" width="330" height="6" rx="3" fill="var(--color-border)" opacity=".4"/>
      <rect x="48" y="54" width="280" height="6" rx="3" fill="var(--color-border)" opacity=".55"/>
      <rect x="48" y="72" width="350" height="6" rx="3" fill="var(--color-border)" opacity=".35"/>
      <rect x="48" y="90" width="190" height="6" rx="3" fill="var(--color-border)" opacity=".5"/>
      <rect x="48" y="108" width="300" height="6" rx="3" fill="var(--color-border)" opacity=".4"/>`;
  }

  #paint() {
    const svg = this.querySelector('.chart-svg');
    if (!svg) return;

    if (this.#error) {
      svg.innerHTML = `<text x="200" y="80" text-anchor="middle" dominant-baseline="middle" fill="var(--color-error-text,currentColor)" font-size="10">${this.#esc(this.#error)}</text>`;
      return;
    }

    const pts = this.#points;
    if (!pts.length) {
      svg.innerHTML = `<text x="200" y="80" text-anchor="middle" dominant-baseline="middle" fill="var(--color-text-muted)" font-size="10">${this.#esc(this.getAttribute('empty-text') || 'No data')}</text>`;
      return;
    }

    const W = 400, H = 160;
    const pad = { t: 8, r: 8, b: 24, l: 48 };
    const gw = W - pad.l - pad.r;
    const gh = H - pad.t - pad.b;

    const xs = pts.map(p => p.x), ys = pts.map(p => p.y);
    const minX = Math.min(...xs), maxX = Math.max(...xs);
    let minY = Math.min(...ys), maxY = Math.max(...ys);
    if (minY === maxY) { minY -= 1; maxY += 1; }
    const rngX = maxX - minX || 1, rngY = maxY - minY;

    const tx = x => pad.l + ((x - minX) / rngX) * gw;
    const ty = y => pad.t + (1 - (y - minY) / rngY) * gh;

    let grid = '', ylbls = '';
    for (let i = 0; i <= 4; i++) {
      const v = minY + (i / 4) * rngY;
      const cy = ty(v).toFixed(1);
      grid += `<line x1="${pad.l}" y1="${cy}" x2="${W - pad.r}" y2="${cy}" stroke="var(--color-border)" stroke-width=".5"/>`;
      ylbls += `<text x="${pad.l - 4}" y="${cy}" dy="4" text-anchor="end" fill="var(--color-text-muted)" font-size="9">${this.#fmtY(v)}</text>`;
    }

    let xlbls = '';
    for (let i = 0; i <= 2; i++) {
      const xv = minX + (i / 2) * (maxX - minX);
      xlbls += `<text x="${tx(xv).toFixed(1)}" y="${H - 2}" text-anchor="middle" fill="var(--color-text-muted)" font-size="9">${this.#fmtX(xv)}</text>`;
    }

    const ptStr = pts.map(p => `${tx(p.x).toFixed(1)},${ty(p.y).toFixed(1)}`).join(' ');
    const bx = tx(pts[0].x).toFixed(1);
    const ex = tx(pts[pts.length - 1].x).toFixed(1);
    const baseY = (pad.t + gh).toFixed(1);
    const areaD = `M${bx},${ty(pts[0].y).toFixed(1)} ` +
      pts.slice(1).map(p => `L${tx(p.x).toFixed(1)},${ty(p.y).toFixed(1)}`).join(' ') +
      ` L${ex},${baseY} L${bx},${baseY} Z`;

    const gid = `lcg${this.#id}`;
    svg.innerHTML = `
      <defs>
        <linearGradient id="${gid}" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color="var(--color-primary)" stop-opacity=".2"/>
          <stop offset="100%" stop-color="var(--color-primary)" stop-opacity=".02"/>
        </linearGradient>
      </defs>
      ${grid}${ylbls}${xlbls}
      <path d="${areaD}" fill="url(#${gid})"/>
      <polyline points="${ptStr}" fill="none" stroke="var(--color-primary)" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round"/>`;
  }

  #fmtY(v) {
    const a = Math.abs(v);
    if (a >= 1e6) return (v / 1e6).toFixed(1) + 'M';
    if (a >= 1e3) return (v / 1e3).toFixed(1) + 'k';
    if (a < 1 && a > 0) return v.toPrecision(2);
    return v.toFixed(v % 1 === 0 ? 0 : 1);
  }

  #fmtX(ms) {
    // HH:MM in IST (UTC+5:30 = +19800000 ms)
    const d = new Date(+ms + 19_800_000);
    return d.toISOString().slice(11, 16);
  }

  #esc(v) {
    return String(v ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }
}

customElements.define('app-line-chart', AppLineChart);
