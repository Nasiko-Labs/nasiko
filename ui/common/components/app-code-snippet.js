/**
 * Labeled command/code block with a copy-to-clipboard button.
 *
 * @element app-code-snippet
 * @attr {string} label - Optional label rendered above the code well.
 * @attr {string} code - The command text. Falls back to the element's initial
 *                       text content when the attribute is absent.
 * @note NightOwl: sand-50 well, r8, Chivo Mono command text, ghost copy button.
 */
import { icons } from '../utils/icons.js';
import { showToast } from '../utils/toast.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-code-snippet) {
  :scope {
    display: block;
    min-width: 0;
  }

  .snippet-label {
    display: block;
    font-size: var(--font-size-sm);
    font-weight: 500;
    color: var(--color-text-main);
    margin-bottom: var(--space-xs);
  }

  .snippet-well {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    background: light-dark(var(--sand-50), var(--neutral-800));
    border-radius: var(--r-8);
    padding: var(--s-8) var(--s-8) var(--s-8) var(--s-16);
    min-height: var(--control-h-lg);
  }

  .snippet-code {
    flex: 1;
    min-width: 0;
    margin: 0;
    padding: 0;
    background: transparent;
    border: none;
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 20px;
    color: var(--color-text-main);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .snippet-copy {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: var(--control-h-sm);
    height: var(--control-h-sm);
    border-radius: var(--r-6);
    color: var(--color-text-muted);
    background: transparent;
    cursor: pointer;

    &:hover { background: light-dark(var(--sand-100), var(--neutral-700)); color: var(--color-text-main); }
    &:focus-visible { outline: 2px solid var(--color-primary); outline-offset: 1px; }
    & svg { width: 14px; height: 14px; }
    &.is-copied { color: var(--color-success); }
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class AppCodeSnippet extends HTMLElement {
  #initialized = false;
  #resetTimer = null;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    const code = this.getAttribute('code') ?? this.textContent.trim();
    const label = this.getAttribute('label');
    this.textContent = '';

    this.innerHTML = `
      ${label ? `<span class="snippet-label"></span>` : ''}
      <div class="snippet-well">
        <pre class="snippet-code"></pre>
        <button class="snippet-copy" type="button" aria-label="Copy to clipboard">${icons.copy('', 14)}</button>
      </div>
    `;
    if (label) this.querySelector('.snippet-label').textContent = label;
    this.querySelector('.snippet-code').textContent = code;

    this.querySelector('.snippet-copy').addEventListener('click', async () => {
      const btn = this.querySelector('.snippet-copy');
      try {
        await navigator.clipboard.writeText(code);
        showToast('Copied to clipboard');
        btn.classList.add('is-copied');
        btn.innerHTML = icons.check('', 14);
        clearTimeout(this.#resetTimer);
        this.#resetTimer = setTimeout(() => {
          btn.classList.remove('is-copied');
          btn.innerHTML = icons.copy('', 14);
        }, 1500);
      } catch {
        showToast('Copy failed — clipboard unavailable');
      }
    });
  }

  disconnectedCallback() {
    clearTimeout(this.#resetTimer);
  }
}

customElements.define('app-code-snippet', AppCodeSnippet);
