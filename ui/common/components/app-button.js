/**
 * Styled button with variants, sizes, loading spinner, and disabled state.
 *
 * @element app-button
 * @attr {string} variant - Visual style: `primary` (default) | `secondary` | `ghost` | `danger`
 * @attr {string} size - Size modifier: `sm` | (default medium)
 * @attr {boolean} disabled - Disables the button
 * @attr {boolean} loading - Shows a spinner and disables the button
 * @attr {string} type - HTML button type: `button` (default) | `submit` | `reset`
 * @prop {boolean} disabled - Get/set disabled state
 * @note Content goes in the default slot.
 */
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-button) {
  :scope {
    display: inline-block;
  }

  :scope[block] {
    display: block;
  }

  .btn {
    width: 100%;
    padding: var(--space-xs) var(--space-md);
    border-radius: var(--radius-md);
    font: 500 var(--font-size-sm)/1 inherit;
    border: 1px solid transparent;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-xs);
    white-space: nowrap;
    min-height: 44px;

    &.is-sm {
      min-height: 36px;
      padding: 5px var(--space-sm);
      font-size: var(--font-size-xs);
    }

    &.is-lg {
      min-height: 52px;
      padding: var(--space-sm) var(--space-lg);
      font-size: var(--font-size-base);
    }

    &.is-primary {
      background: var(--color-primary);
      color: var(--color-on-primary);

      &:hover:not(:disabled) {
        background: var(--color-primary-hover);
      }
    }

    &.is-secondary {
      background: var(--color-bg-surface);
      border-color: var(--color-border);
      color: var(--color-text-main);

      &:hover:not(:disabled) {
        background: var(--color-bg-base);
        border-color: var(--color-text-muted);
      }
    }

    &.is-ghost {
      background: transparent;
      color: var(--color-text-muted);

      &:hover:not(:disabled) {
        background: var(--color-bg-base);
        color: var(--color-text-main);
      }
    }

    &.is-outline {
      background: color-mix(in srgb, var(--color-primary) 12%, var(--color-bg-surface));
      color: var(--color-text-main);
      border: 2px solid var(--color-primary-hover);

      &:hover:not(:disabled) {
        background: var(--color-primary-hover);
        color: var(--color-on-primary);
      }
    }

    &.is-danger {
      background: var(--color-error);
      color: var(--color-on-primary);

      &:hover:not(:disabled) {
        background: color-mix(in srgb, var(--color-error) 85%, black);
      }
    }

    &.is-icon {
      width: 2rem;
      height: 2rem;
      min-height: unset;
      padding: 0;
      background: transparent;
      color: var(--color-text-muted);
      border-color: transparent;
      border-radius: var(--radius-sm);

      &:hover:not(:disabled) {
        background: var(--color-bg-base);
        color: var(--color-text-main);
      }
    }

    &:disabled {
      opacity: 0.5;
    }
  }

  @keyframes btn-pulse {

    0%,
    100% {
      opacity: 1;
    }

    50% {
      opacity: 0.2;
    }
  }

  .spinner {
    width: 0.6em;
    height: 0.6em;
    border-radius: 50%;
    flex-shrink: 0;
    background: currentColor;
    animation: btn-pulse 1s ease-in-out infinite;
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];


export class AppButton extends HTMLElement {
  static get observedAttributes() { return ['variant', 'size', 'disabled', 'loading', 'type']; }
  constructor() { super(); }
  get disabled() { return this.hasAttribute('disabled'); }
  set disabled(val) { val ? this.setAttribute('disabled', '') : this.removeAttribute('disabled'); }
  connectedCallback() { this.render(); }
  attributeChangedCallback() { if (this.isConnected) this.render(); }

  render() {
    const variant  = this.getAttribute('variant') || 'primary';
    const size     = this.getAttribute('size') || '';
    const type     = this.getAttribute('type') || 'button';
    const loading  = this.hasAttribute('loading');
    const disabled = this.hasAttribute('disabled') || loading;
    const content  = this.querySelector('.content')?.innerHTML ?? this.innerHTML;
    const classes  = ['btn', `is-${variant}`, size ? `is-${size}` : ''].filter(Boolean).join(' ');

    this.innerHTML = `
      <button class="${classes}" type="${type}"${disabled ? ' disabled' : ''}>
        ${loading ? '<span class="spinner" aria-hidden="true"></span>' : ''}
        <span class="content">${content}</span>
      </button>`;
  }
}
customElements.define('app-button', AppButton);

