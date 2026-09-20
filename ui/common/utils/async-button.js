/**
 * Wraps an async action with button loading state.
 * Disables the button, swaps text to `loadingText`, restores on completion.
 *
 *   import { withLoading } from '/common/utils/async-button.js';
 *   btn.addEventListener('click', withLoading(btn, 'Saving…', async () => { ... }));
 */
export function withLoading(btn, loadingText, fn) {
  return async (...args) => {
    const original = btn.textContent;
    btn.disabled = true;
    btn.textContent = loadingText;
    try {
      return await fn(...args);
    } finally {
      btn.disabled = false;
      btn.textContent = original;
    }
  };
}
