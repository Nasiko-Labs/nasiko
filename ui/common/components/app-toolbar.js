/**
 * Horizontal toolbar with named start and end slots for grouping actions.
 *
 * @element app-toolbar
 * @attr {string} aria-label - Accessible label for the toolbar region
 * @slot [data-slot="start"] - Left-side content
 * @slot [data-slot="end"] - Right-side content
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-toolbar) {
    .toolbar {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: var(--space-md);
      flex-wrap: wrap;
      padding: var(--space-sm) 0;
    }
    .start { display: flex; align-items: center; gap: var(--space-sm); flex: 1; min-width: 0; flex-wrap: wrap; }
    .end   { display: flex; align-items: center; gap: var(--space-sm); flex-shrink: 0; flex-wrap: wrap; }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];


export class AppToolbar extends HTMLElement {
  constructor() { super(); }

  connectedCallback() {
    if (this._initialized) return;
    this._initialized = true;

    const start = [...this.children].find(el => el.dataset.slot === 'start');
    const end   = [...this.children].find(el => el.dataset.slot === 'end');

    const wrap     = document.createElement('div');
    const startDiv = Object.assign(document.createElement('div'), { className: 'start' });
    const endDiv   = Object.assign(document.createElement('div'), { className: 'end' });

    wrap.className = 'toolbar';
    wrap.setAttribute('role', 'toolbar');
    wrap.setAttribute('aria-label', this.getAttribute('aria-label') ?? 'Toolbar');
    if (start) startDiv.appendChild(start);
    if (end)   endDiv.appendChild(end);
    wrap.append(startDiv, endDiv);
    this.appendChild(wrap);
  }
}
customElements.define('app-toolbar', AppToolbar);
