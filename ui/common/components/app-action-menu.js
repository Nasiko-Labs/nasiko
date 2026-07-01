/**
 * Icon button that opens a compact dropdown list of actions.
 *
 * Pass the trigger icon as innerHTML and configure items via the `items` attribute.
 * The component captures the inner HTML on first connect as the trigger icon.
 *
 * @example
 * ```html
 * <app-action-menu trigger-title="Options" items='[{"id":"a","label":"Action A"},{"id":"b","label":"Action B"}]'>
 *   <svg ...></svg>
 * </app-action-menu>
 * ```
 *
 * @element app-action-menu
 * @attr {string} trigger-title - Tooltip text for the trigger button
 * @attr {string} items - JSON array of `{ id, label }` action items
 * @fires action-select - Item clicked; `detail: { id: string }` — bubbles
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-action-menu) {
  :scope {
    position: relative;
    display: inline-flex;
  }

  .aam-trigger svg {
    width: 18px;
    height: 18px;
  }

  .aam-menu {
    position: absolute;
    top: calc(100% + 2px);
    left: 0;
    min-width: 10rem;
    background: var(--color-bg-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-md);
    z-index: 200;
    padding: var(--space-xs);
  }

  .aam-item {
    display: block;
    width: 100%;
    text-align: left;
    padding: var(--space-xs) var(--space-sm);
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    color: var(--color-text-main);
    white-space: nowrap;

    &:hover {
      background: color-mix(in srgb, var(--color-primary) 8%, transparent);
      color: var(--color-primary);
    }
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export class AppActionMenu extends HTMLElement {
  #open = false;
  #outsideClickHandler = null;
  #escHandler = (e) => { if (e.key === 'Escape') this.#close(); };

  connectedCallback() {
    const iconHtml = this.innerHTML.trim();
    const items = JSON.parse(this.getAttribute('items') || '[]');
    const title = this.getAttribute('trigger-title') || '';

    this.innerHTML = `
      <button class="aam-trigger btn-icon" title="${title}" aria-haspopup="true" aria-expanded="false">
        ${iconHtml}
      </button>
      <div class="aam-menu" hidden role="menu">
        ${items.map(item => `<button class="aam-item" role="menuitem" data-id="${item.id}">${item.label}</button>`).join('')}
      </div>
    `;

    this.#bindEvents();
  }

  disconnectedCallback() {
    this.#close();
  }

  #bindEvents() {
    this.querySelector('.aam-trigger').addEventListener('click', (e) => {
      e.stopPropagation();
      this.#open ? this.#close() : this.#openMenu();
    });

    this.querySelectorAll('.aam-item').forEach(btn => {
      btn.addEventListener('click', () => {
        this.#close();
        this.dispatchEvent(new CustomEvent('action-select', {
          bubbles: true,
          detail: { id: btn.dataset.id },
        }));
      });
    });
  }

  #openMenu() {
    this.#open = true;
    this.querySelector('.aam-menu').hidden = false;
    this.querySelector('.aam-trigger').setAttribute('aria-expanded', 'true');

    this.#outsideClickHandler = (e) => { if (!this.contains(e.target)) this.#close(); };
    document.addEventListener('click', this.#outsideClickHandler);
    document.addEventListener('keydown', this.#escHandler);
  }

  #close() {
    if (!this.#open) return;
    this.#open = false;
    const menu = this.querySelector('.aam-menu');
    const trigger = this.querySelector('.aam-trigger');
    if (menu) menu.hidden = true;
    if (trigger) trigger.setAttribute('aria-expanded', 'false');

    document.removeEventListener('click', this.#outsideClickHandler);
    document.removeEventListener('keydown', this.#escHandler);
    this.#outsideClickHandler = null;
  }
}

customElements.define('app-action-menu', AppActionMenu);
