/**
 * Global toast notification manager — import to mount the container and expose `window.toast`.
 *
 * @global window.toast - Toast notification manager
 * @global toast.success(message, duration?) - Show a success toast (default 3 s)
 * @global toast.error(message, duration?) - Show an error toast
 * @global toast.info(message, duration?) - Show an info toast
 * @note Not a custom element. Import this file to mount the toast container and expose `window.toast`.
 */
import { icons } from '../utils/icons.js';
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (.app-toast-container) {
  :scope {
    position: fixed;
    bottom: var(--space-lg);
    right: var(--space-lg);
    z-index: 9999;
    display: flex;
    flex-direction: column;
    gap: var(--space-sm);
    pointer-events: none;
  }
  .app-toast {
    padding: var(--space-sm) var(--space-md);
    border-radius: var(--radius-md);
    background-color: var(--color-bg-surface);
    color: var(--color-text-main);
    box-shadow: var(--shadow-lg);
    border: 1px solid var(--color-border);
    font-size: var(--font-size-sm);
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    max-width: 320px;

    &.is-success { border-left: 4px solid var(--color-success); }
    &.is-error   { border-left: 4px solid var(--color-error); }
    &.is-info    { border-left: 4px solid var(--color-primary); }
  }
}
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class ToastManager {
  constructor() {
    this.container = document.createElement('div');
    this.container.className = 'app-toast-container';
    document.body.appendChild(this.container);
  }

  show(message, type = 'info', duration = 3000) {
    const toast = document.createElement('div');
    toast.className = `app-toast is-${type}`;
    toast.innerHTML = `
      ${this.getIcon(type)}
      <span class="message">${this.escapeHtml(message)}</span>
    `;
    this.container.appendChild(toast);
    setTimeout(() => toast.remove(), duration);
  }

  getIcon(type) {
    switch (type) {
      case 'success': return icons.check('', 18);
      case 'error':   return icons.xCircle('', 18);
      default:        return icons.info('', 18);
    }
  }

  escapeHtml(str) {
    return str
      .replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/>/g, '&gt;').replace(/"/g, '&quot;')
      .replace(/'/g, '&#039;');
  }
}

let manager = null;

export const toast = {
  show:    (message, type, duration) => { if (!manager) manager = new ToastManager(); manager.show(message, type, duration); },
  success: (message, duration) => toast.show(message, 'success', duration),
  error:   (message, duration) => toast.show(message, 'error', duration),
  info:    (message, duration) => toast.show(message, 'info', duration),
};
