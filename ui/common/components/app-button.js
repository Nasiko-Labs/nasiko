/**
 * Styled button with variants, sizes, loading spinner, and disabled state.
 *
 * @element app-button
 * @attr {string} variant - Visual style: `primary` (default) | `secondary` | `ghost` | `danger` | `dark`
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
    padding: 0 var(--s-16);
    border-radius: var(--r-8);
    font: 500 var(--font-size-sm)/1 inherit;
    border: 1px solid transparent;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-xs);
    white-space: nowrap;
    min-height: var(--control-h-lg);
    transition: background var(--transition-fast), color var(--transition-fast);

    &.is-sm {
      min-height: var(--control-h-sm);
      padding: 0 var(--s-12);
      font-size: 13px;
    }

    &.is-lg {
      min-height: 44px;
      padding: 0 var(--space-lg);
      font-size: var(--font-size-base);
    }

    &.is-primary {
      background: light-dark(var(--sand-800), var(--neutral-100));
      color: light-dark(var(--white), var(--neutral-900));

      &:hover:not(:disabled) {
        background: light-dark(var(--sand-700), var(--neutral-300));
      }
    }

    &.is-secondary {
      background: light-dark(var(--yellow-100), var(--yellow-900));
      border-color: light-dark(var(--yellow-200), var(--yellow-800));
      color: var(--color-text-main);

      &:hover:not(:disabled) {
        background: light-dark(var(--yellow-200), var(--yellow-800));
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
      background: var(--color-bg-surface);
      color: var(--color-text-main);
      border: 1px solid var(--color-border);

      &:hover:not(:disabled) {
        background: var(--color-bg-base);
      }
    }

    &.is-danger {
      background: var(--color-error);
      color: var(--white);

      &:hover:not(:disabled) {
        background: color-mix(in srgb, var(--color-error) 85%, black);
      }
    }

    &.is-dark {
      background: light-dark(var(--sand-800), var(--neutral-100));
      color: light-dark(var(--white), var(--neutral-900));

      &:hover:not(:disabled) {
        background: light-dark(var(--sand-700), var(--neutral-300));
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

    /* Global reset sets svg { display: block } — lay the slotted content out
       as inline flex so icon + label sit on one line with a consistent gap. */
    .content {
      display: inline-flex;
      align-items: center;
      gap: var(--space-xs);
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

  /// Replace the button's text.
  ///
  /// Assigning to `element.textContent` directly would wipe the rendered
  /// `<button>` wrapper and leave a bare text node — the button keeps its box
  /// in the layout but loses all of its styling. Callers that relabel a button
  /// (e.g. a Create/Save-changes modal) should use this instead.
  set label(text) {
    const content = this.querySelector('.content');
    if (content) content.textContent = text;
    else this.textContent = text; // not rendered yet; render() picks this up
  }

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

