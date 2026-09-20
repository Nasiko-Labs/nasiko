import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';

/**
 * Type-to-search single-user picker backed by `GET /api/search/users`.
 *
 * Used wherever a form assigns a person to a role — department manager, team
 * lead — so the search/debounce/select/clear behaviour lives in one place
 * instead of once per form.
 *
 * @attr {string} label       - Field label. Omit for no label.
 * @attr {string} placeholder - Search input placeholder.
 * @attr {string} empty-text  - Message when a search returns nothing.
 * @prop {?{id: string, name: string}} value - Selected user, or null.
 * @fires change - After the selection changes, either way. `detail` is the new value.
 */

const MIN_QUERY_LENGTH = 2;
const DEBOUNCE_MS = 250;

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (user-picker) {
  :scope { display: flex; flex-direction: column; gap: 3px; }
  /* Deliberately a bare label/input: a host form's own field styling
     (e.g. a .modal-form label rule) is more specific than these, so the picker
     matches its sibling fields inside a form and still looks right on its own. */
  label {
    font-size: var(--font-size-sm);
    font-weight: 500;
    color: var(--color-text-main);
  }
  .up-picked { margin-bottom: var(--space-xs); }
  .up-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-xs);
    padding: 2px var(--space-sm);
    border-radius: 999px;
    background: var(--bg-input);
    font-size: var(--font-size-sm);
  }
  .up-chip-x {
    display: inline-flex;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--color-text-muted);
    cursor: pointer;
  }
  .up-chip-x:hover { color: var(--color-text-main); }
  .up-query {
    width: 100%;
    height: var(--control-h-md);
    padding: 0 var(--s-12);
    border: 1px solid transparent;
    border-radius: var(--r-8);
    background-color: var(--bg-input);
    color: var(--color-text-main);
    font-family: inherit;
    font-size: var(--font-size-sm);
  }
  .up-query:focus {
    outline: none;
    border-color: var(--border-hover);
    box-shadow: 0 0 0 2px var(--color-primary-ring);
  }
  .up-results {
    max-height: 180px;
    overflow-y: auto;
    margin-top: var(--space-xs);
    border: 1px solid var(--color-border);
    border-radius: var(--r-8);
  }
  .up-row {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
    padding: var(--space-xs) var(--space-sm);
    border: none;
    border-bottom: 1px solid color-mix(in srgb, var(--color-border) 50%, transparent);
    background: transparent;
    color: var(--color-text-main);
    font-family: inherit;
    font-size: var(--font-size-sm);
    text-align: left;
    cursor: pointer;
  }
  .up-row:last-child { border-bottom: none; }
  .up-row:hover { background: var(--bg-surface-hover); }
  .up-row:focus-visible { outline: 2px solid var(--color-primary); outline-offset: -2px; }
  .up-sub { color: var(--color-text-muted); font-size: var(--font-size-xs); }
  .up-empty {
    padding: var(--space-sm);
    color: var(--color-text-muted);
    font-size: var(--font-size-sm);
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class UserPicker extends HTMLElement {
  #value = null;
  #debounce = null;
  /** Guards against a slow earlier search overwriting a newer one's results. */
  #searchSeq = 0;

  connectedCallback() {
    if (this.querySelector('.up-query')) return; // already rendered
    this.#render();
  }

  disconnectedCallback() {
    clearTimeout(this.#debounce);
  }

  get value() {
    return this.#value;
  }

  /** Set (or clear) the selection without emitting `change`. */
  set value(next) {
    this.#value = next && next.id ? { id: next.id, name: next.name || next.id } : null;
    this.#renderPicked();
  }

  /** Clears the selection and any in-progress search. Does not emit `change`. */
  reset() {
    this.value = null;
    const query = this.querySelector('.up-query');
    if (query) query.value = '';
    this.#hideResults();
  }

  #render() {
    const label = this.getAttribute('label');
    const placeholder = this.getAttribute('placeholder') || 'Search users by name or email';

    this.innerHTML = `
      ${label ? `<label>${this.#esc(label)}</label>` : ''}
      <div class="up-picked" hidden></div>
      <input type="search" class="up-query" placeholder="${this.#escAttr(placeholder)}"
        autocomplete="off" role="combobox" aria-expanded="false" aria-autocomplete="list" />
      <div class="up-results" role="listbox" hidden></div>
    `;

    this.querySelector('.up-query').addEventListener('input', (e) => {
      clearTimeout(this.#debounce);
      const term = e.target.value;
      this.#debounce = setTimeout(() => this.#search(term), DEBOUNCE_MS);
    });

    this.#renderPicked();
  }

  async #search(rawQuery) {
    const query = rawQuery.trim();
    if (query.length < MIN_QUERY_LENGTH) {
      this.#hideResults();
      return;
    }

    const seq = ++this.#searchSeq;
    let users = [];
    try {
      const res = await apiFetch(`/search/users?q=${encodeURIComponent(query)}`);
      if (res.ok) users = (await res.json())?.data ?? [];
    } catch {
      // Treated as "no matches" — a transient search failure shouldn't wipe a
      // selection the user already made.
    }
    if (seq !== this.#searchSeq) return; // a newer search superseded this one

    this.#renderResults(users);
  }

  #renderResults(users) {
    const results = this.querySelector('.up-results');
    if (!users.length) {
      const emptyText = this.getAttribute('empty-text') || 'No matching users';
      results.innerHTML = `<div class="up-empty">${this.#esc(emptyText)}</div>`;
      this.#showResults();
      return;
    }

    results.innerHTML = users.map((user) => {
      const name = user.display_name || user.username || user.id;
      return `
        <button type="button" class="up-row" role="option"
          data-id="${this.#escAttr(user.id)}" data-name="${this.#escAttr(name)}">
          <span>${this.#esc(name)}</span>
          ${user.email ? `<span class="up-sub">${this.#esc(user.email)}</span>` : ''}
        </button>`;
    }).join('');

    results.querySelectorAll('.up-row').forEach((row) => {
      row.addEventListener('click', () => {
        this.#select({ id: row.dataset.id, name: row.dataset.name });
      });
    });
    this.#showResults();
  }

  #select(user) {
    this.value = user;
    const query = this.querySelector('.up-query');
    query.value = '';
    this.#hideResults();
    this.dispatchEvent(new CustomEvent('change', { detail: this.#value, bubbles: true }));
  }

  #renderPicked() {
    const picked = this.querySelector('.up-picked');
    if (!picked) return;

    if (!this.#value) {
      picked.hidden = true;
      picked.innerHTML = '';
      return;
    }

    picked.innerHTML = `
      <span class="up-chip">${this.#esc(this.#value.name)}
        <button type="button" class="up-chip-x" aria-label="Clear selection">${icons.x('', 12)}</button>
      </span>`;
    picked.hidden = false;
    picked.querySelector('.up-chip-x').addEventListener('click', () => {
      this.value = null;
      this.dispatchEvent(new CustomEvent('change', { detail: null, bubbles: true }));
    });
  }

  #showResults() {
    this.querySelector('.up-results').hidden = false;
    this.querySelector('.up-query').setAttribute('aria-expanded', 'true');
  }

  #hideResults() {
    const results = this.querySelector('.up-results');
    if (results) results.hidden = true;
    const query = this.querySelector('.up-query');
    if (query) query.setAttribute('aria-expanded', 'false');
  }

  #esc(str) {
    const d = document.createElement('span');
    d.textContent = str ?? '';
    return d.innerHTML;
  }

  #escAttr(str) {
    return String(str ?? '').replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('user-picker', UserPicker);
