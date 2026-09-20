/**
 * Horizontal flex row with configurable gap, alignment, and optional wrapping.
 *
 * @element app-row
 * @attr {string} gap - Space between items: `xs` | `sm` | `md` (default) | `lg` | `xl`
 * @attr {string} align - Cross-axis alignment: `start` | `center` | `end` | `stretch` (default)
 * @attr {string} justify - Main-axis alignment: `start` (default) | `center` | `end` | `between`
 * @attr {string} padding - Inner padding token: `xs` | `sm` | `md` | `lg` | `xl`
 * @attr {boolean} wrap - Allow items to wrap to next line
 * @note Horizontal flex row. For vertical use `<app-stack>`.
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-row) {
  :scope {
    display: flex;
    flex-direction: row;
    gap: var(--row-gap, var(--space-md));
    align-items: var(--row-align, center);
    justify-content: var(--row-justify, flex-start);
    padding: var(--row-padding, 0);
    flex-wrap: var(--row-wrap, nowrap);
    width: 100%;
  }
}`);
import { BaseLayout } from './base-layout.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export class AppRow extends BaseLayout {
  static get observedAttributes() { return ['gap', 'align', 'justify', 'padding', 'wrap']; }
  constructor() { super('row'); }
}
customElements.define('app-row', AppRow);
