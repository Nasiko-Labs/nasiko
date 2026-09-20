/**
 * Small inline label with color variants for status, category, or metadata.
 *
 * @element app-badge
 * @attr {string} variant - Visual style: `neutral` (default) | `success` | `warning` | `error` | `info`
 * @note CSS-only component — no JS logic. Content goes in the default slot.
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-badge) {
    :scope {
      display: inline-flex;
      align-items: center;
      gap: 4px;
      padding: 2px var(--space-xs);
      border-radius: 999px;
      font-size: var(--font-size-xs);
      font-weight: 500;
      line-height: 1.4;
      white-space: nowrap;
      border: 1px solid;

      &[variant="success"] { color: var(--color-success); background: var(--color-success-bg); border-color: var(--color-success-border); }
      &[variant="warning"] { color: var(--color-warning); background: var(--color-warning-bg); border-color: var(--color-warning-border); }
      &[variant="error"]   { color: var(--color-error);   background: var(--color-error-bg);   border-color: var(--color-error-border); }
      &[variant="info"]    { color: var(--color-info);    background: var(--color-info-bg);     border-color: var(--color-info-border); }
      &[variant="neutral"] { color: var(--color-neutral); background: var(--color-neutral-bg);  border-color: var(--color-neutral-border); }

      &[dot]::before {
        content: "";
        width: 6px;
        height: 6px;
        border-radius: 50%;
        background: currentColor;
        flex-shrink: 0;
      }
    }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];


export class AppBadge extends HTMLElement {}
customElements.define('app-badge', AppBadge);
