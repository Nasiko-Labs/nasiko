import '/common/components/app-modal.js';

const sheet = new CSSStyleSheet();
sheet.replaceSync(`
  .confirm-dialog-body {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin: 0;
    line-height: 1.5;
  }
  .confirm-dialog-footer {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-sm);
  }
  .confirm-btn, .cancel-btn {
    display: inline-flex;
    align-items: center;
    padding: 8px var(--space-md);
    border-radius: var(--r-8);
    font-size: var(--font-size-sm);
    font-weight: 500;
    cursor: pointer;
  }
  .cancel-btn {
    border: 1px solid var(--color-border);
    background: var(--color-bg-surface);
    color: var(--color-text-muted);
  }
  .cancel-btn:hover { background: var(--bg-input); }
  .confirm-btn {
    border: none;
    background: var(--color-text-main);
    color: var(--color-bg-surface);
  }
  .confirm-btn:hover { opacity: 0.9; }
  .confirm-btn.danger {
    background: var(--color-error, #ef4444);
    color: #fff;
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];

/**
 * Shows an in-app confirmation dialog (replaces browser `confirm()`).
 * Returns a Promise that resolves `true` on confirm, `false` on cancel/close.
 *
 * @param {object} opts
 * @param {string} opts.title - Modal heading
 * @param {string} opts.message - Body text (supports HTML)
 * @param {string} [opts.confirmLabel='Confirm'] - Primary button label
 * @param {string} [opts.cancelLabel='Cancel'] - Secondary button label
 * @param {boolean} [opts.danger=false] - Styles the confirm button as destructive
 */
export function confirmDialog({
  title,
  message,
  confirmLabel = 'Confirm',
  cancelLabel = 'Cancel',
  danger = false,
}) {
  return new Promise((resolve) => {
    const modal = document.createElement('app-modal');
    modal.setAttribute('heading', title);

    modal.innerHTML = `
      <p class="confirm-dialog-body">${message}</p>
      <div data-slot="footer" class="confirm-dialog-footer">
        <button type="button" class="cancel-btn" data-role="cancel">${cancelLabel}</button>
        <button type="button" class="confirm-btn${danger ? ' danger' : ''}" data-role="confirm">${confirmLabel}</button>
      </div>
    `;

    document.body.appendChild(modal);
    let resolved = false;

    const cleanup = (result) => {
      if (resolved) return;
      resolved = true;
      modal.close();
      modal.remove();
      resolve(result);
    };

    modal.querySelector('[data-role="cancel"]').addEventListener('click', () => cleanup(false));
    modal.querySelector('[data-role="confirm"]').addEventListener('click', () => cleanup(true));
    modal.querySelector('dialog')?.addEventListener('close', () => cleanup(false));

    modal.open();
  });
}
