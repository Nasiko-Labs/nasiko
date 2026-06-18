/**
 * Animated shimmer placeholder for content that is still loading.
 *
 * @element app-skeleton
 * @attr {number} lines - Number of skeleton lines to render (default: 3)
 * @attr {string} height - Height of each line (CSS value, e.g. `1rem`)
 * @attr {string} radius - Border radius of each line (CSS value)
 * @note Used as a loading placeholder. Animates with a shimmer effect.
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@keyframes skeleton-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.35; } }
  @media (prefers-reduced-motion: reduce) { @keyframes skeleton-pulse { from, to { opacity: 1; } } }
  @scope (app-skeleton) {
    .skel {
      background: var(--color-border);
      border-radius: var(--radius-sm);
      animation: skeleton-pulse 1.5s ease-in-out infinite;

      &.is-line {
        height: 0.85em;
        margin-bottom: var(--space-xs);
        &:last-child { width: 65%; }
      }
    }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export class AppSkeleton extends HTMLElement {
  constructor() { super(); }
  connectedCallback() { if (this._initialized) return; this._initialized = true; this.render(); }
  attributeChangedCallback() { if (this.isConnected) this.render(); }
  render() {
    const lines  = parseInt(this.getAttribute('lines') || '0', 10);
    const height = this.getAttribute('height') || '1rem';
    const radius = this.getAttribute('radius') || 'sm';
    this.innerHTML = lines > 0
      ? Array.from({ length: lines }, () => `<div class="skel is-line"></div>`).join('')
      : `<div class="skel" style="height:${height};border-radius:var(--radius-${radius});width:100%"></div>`;
  }
}
customElements.define('app-skeleton', AppSkeleton);
