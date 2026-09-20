/**
 * Placeholder shown when a list or view has no content yet.
 *
 * @element app-empty-state
 * @attr {string} title - Bold heading text
 * @attr {string} description - Supporting description text
 * @attr {string} icon - SVG markup string for the icon (alternative to slot)
 * @slot [slot="icon"] - Element to use as the icon
 * @slot default - Action elements (e.g. a button)
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-empty-state) {
    :scope {
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: var(--space-md);
      padding: var(--space-2xl) var(--space-xl);
      text-align: center;
      border: 1px dashed var(--color-border);
      border-radius: var(--radius-lg);
      background: var(--color-bg-surface);
    }
    .icon {
      color: var(--color-text-muted);
      line-height: 1;
      & svg { width: 2.5rem; height: 2.5rem; }
    }
    .title { margin: 0; font-size: var(--font-size-lg); font-weight: 600; }
    .desc  { margin: 0; color: var(--color-text-muted); font-size: var(--font-size-sm); max-width: 40ch; }
    .action { display: flex; gap: var(--space-sm); flex-wrap: wrap; justify-content: center; }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];


export class AppEmptyState extends HTMLElement {
  constructor() { super(); }
  connectedCallback() {
    if (this._initialized) return;
    this._initialized = true;
    const children = [...this.children];
    const iconChild = children.find(n => n.getAttribute?.('slot') === 'icon');
    const actions = children.filter(n => n.getAttribute?.('slot') !== 'icon');
    const title    = this.getAttribute('title') || '';
    const desc     = this.getAttribute('description') || '';
    const iconAttr = this.getAttribute('icon') || '';
    const hasIcon  = iconAttr || iconChild;
    // `title` collides with the global HTML tooltip attribute — every
    // browser would show it as a native hover tooltip duplicating the
    // heading text below. Drop it from the DOM once consumed; it's only
    // ever read here (no observedAttributes reactivity to preserve it for).
    this.removeAttribute('title');
    this.innerHTML = `
      ${hasIcon ? `<div class="icon">${iconAttr}</div>` : ''}
      ${title ? `<p class="title">${title}</p>` : ''}
      ${desc  ? `<p class="desc">${desc}</p>`   : ''}
      <div class="action"></div>`;
    if (iconChild) {
      const iconSlot = this.querySelector('.icon');
      if (iconSlot) iconSlot.appendChild(iconChild);
    }
    const slot = this.querySelector('.action');
    actions.forEach(n => slot.appendChild(n));
  }
}
customElements.define('app-empty-state', AppEmptyState);
