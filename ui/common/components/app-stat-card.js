/**
 * Metric summary card showing a label, primary value, delta, and trend direction.
 *
 * @element app-stat-card
 * @attr {string} label - Metric label (e.g. "Total Revenue")
 * @attr {string} value - Primary value to display
 * @attr {string} delta - Change value shown below the main value (e.g. "+12%")
 * @attr {string} trend - Trend direction: `up` | `down` | `neutral`
 * @attr {boolean} loading - Show a skeleton placeholder instead of data
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@keyframes skel-pulse {
  0%, 100% { opacity: 1; }
  50%       { opacity: 0.45; }
}
@media (prefers-reduced-motion: reduce) {
  @keyframes skel-pulse { from, to { opacity: 1; } }
}

@scope (app-stat-card) {
    /* Surface: common/styles/surface.css (.stat-card is listed there). This sheet
       is adopted on document, so it cascades after the linked sheets — declaring
       the surface here would silently fork it. Layout only. */
    .stat-card {
      padding: var(--space-lg);
    }
    .stat-card.is-loading {
      min-height: 100px;
      background: var(--color-border);
      border: none;
      box-shadow: none;
      animation: skel-pulse 1.5s ease-in-out infinite;
    }
    @media (prefers-reduced-motion: reduce) {
      .stat-card.is-loading { animation: none; opacity: 0.6; }
    }
    .label { margin: 0 0 var(--space-xs); color: var(--color-text-muted); font-size: var(--font-size-sm); }
    .value { margin: 0; font-size: var(--font-size-3xl); font-weight: 700; line-height: 1.1; }
    .delta {
      margin: var(--space-xs) 0 0;
      font-size: var(--font-size-sm);
      font-weight: 500;

      &.is-up      { color: var(--color-success); }
      &.is-down    { color: var(--color-error); }
      &.is-neutral { color: var(--color-text-muted); }
    }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];


export class AppStatCard extends HTMLElement {
  static get observedAttributes() { return ['label', 'value', 'delta', 'trend', 'loading']; }
  constructor() { super(); }
  connectedCallback() { if (this._initialized) return; this._initialized = true; this.render(); }
  attributeChangedCallback() { if (this.isConnected) this.render(); }
  render() {
    if (this.hasAttribute('loading')) {
      this.innerHTML = '<div class="stat-card is-loading"></div>';
      return;
    }
    const label = this.getAttribute('label') || '';
    const value = this.getAttribute('value') || '—';
    const delta = this.getAttribute('delta') || '';
    const trend = this.getAttribute('trend') || 'neutral';
    this.innerHTML = `
      <div class="stat-card">
        <p class="label">${label}</p>
        <p class="value">${value}</p>
        ${delta ? `<p class="delta is-${trend}">${delta}</p>` : ''}
      </div>`;
  }
}
customElements.define('app-stat-card', AppStatCard);
