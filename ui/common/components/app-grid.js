/**
 * CSS grid layout wrapper with configurable columns, gap, and padding.
 *
 * @element app-grid
 * @attr {string|number} columns - Column count (integer) or CSS grid-template value (e.g. `auto-fit`)
 * @attr {string} min-width - Minimum column width for auto-fit layouts (e.g. `280px`)
 * @attr {string} gap - Gap between cells: `xs` | `sm` | `md` (default) | `lg` | `xl`
 * @attr {string} padding - Inner padding token: `xs` | `sm` | `md` | `lg` | `xl`
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-grid) {
    :scope {
      display: grid;
      grid-template-columns: var(--grid-columns, repeat(auto-fill, minmax(var(--grid-min-width, 300px), 1fr)));
      gap: var(--grid-gap, var(--space-md));
      padding: var(--grid-padding, 0);
      width: 100%;
    }
  }
`);
import { BaseLayout } from './base-layout.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export class AppGrid extends BaseLayout {
  static get observedAttributes() { return ['columns', 'min-width', 'gap', 'padding']; }
  constructor() { super('grid'); }

  updateProperty(name, value) {
    if (name === 'columns') {
      const n = Number(value);
      const val = (Number.isInteger(n) && n > 0) ? `repeat(${n}, 1fr)` : value;
      this.style.setProperty('--grid-columns', val);
    } else {
      super.updateProperty(name, value);
    }
  }
}
customElements.define('app-grid', AppGrid);
