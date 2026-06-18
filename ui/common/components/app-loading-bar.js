/**
 * Thin top-of-page progress bar driven by `loading-start` / `loading-end` document events.
 *
 * @element app-loading-bar
 * @method show() - Make the bar visible
 * @note Listens to `loading-start` / `loading-end` custom events on `document` automatically.
 * @note Place once in the page (typically inside `<app-header>`).
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-loading-bar) {
    :scope {
      position: fixed;
      top: 0;
      left: 0;
      width: 100vw;
      height: 3px;
      background: var(--color-primary);
      z-index: 9999;
      opacity: 0;
      pointer-events: none;

      &.visible {
        animation: app-loading-bar-pulse 1.2s ease-in-out infinite;
      }
    }
  }
  @keyframes app-loading-bar-pulse {
    0%, 100% { opacity: 0.4; }
    50% { opacity: 1; }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];


export class AppLoadingBar extends HTMLElement {
  #handleLoadingStart = () => this.show();
  #handleLoadingEnd   = () => this.hide();

  constructor() {
    super();
    this.classList.add('app-loading-bar');
  }

  connectedCallback() {
    document.addEventListener('loading-start', this.#handleLoadingStart);
    document.addEventListener('loading-end', this.#handleLoadingEnd);
  }

  disconnectedCallback() {
    document.removeEventListener('loading-start', this.#handleLoadingStart);
    document.removeEventListener('loading-end', this.#handleLoadingEnd);
  }

  show() { this.classList.add('visible'); }
  hide() { this.classList.remove('visible'); }
}

customElements.define('app-loading-bar', AppLoadingBar);
