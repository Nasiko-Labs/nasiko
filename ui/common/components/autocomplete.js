/**
 * Typeahead text input that calls a window function for suggestions and fires on selection.
 *
 * @element auto-complete
 * @attr {string} placeholder - Input placeholder text
 * @attr {string} aria-label - Accessible label for the input
 * @attr {string} filter-function - Name of `window[fn](query)` returning `[{ label, value }]`
 * @fires option-selected - Option chosen; `detail: { value, option }` — bubbles
 */
import { icons } from '../utils/icons.js';
import styles from './autocomplete.css' with { type: 'css' };
import { DropdownController } from './dropdown-controller.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export default class AutoComplete extends HTMLElement {
  #initialized = false;
  #inputEl = null;
  #dropdownEl = null;
  #dd = null;              // DropdownController
  #filteredOptions = [];
  #filterFn = null;

  static get observedAttributes() {
    return ['placeholder', 'aria-label', 'filter-function'];
  }

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#render();
    this.#setupEvents();
    this.#resolveFilterFn();
  }

  attributeChangedCallback(name, _old, val) {
    if (!this.#initialized) return;
    if (name === 'placeholder')      this.#inputEl.placeholder = val || 'Search...';
    if (name === 'aria-label')       this.#inputEl.setAttribute('aria-label', val || 'Search');
    if (name === 'filter-function')  this.#resolveFilterFn();
  }

  #render() {
    this.innerHTML = `
      <div class="ac-container">
        <div class="ac-input-wrapper">
          <input type="text"
                 class="ac-input"
                 placeholder="${this.getAttribute('placeholder') || 'Search...'}"
                 aria-label="${this.getAttribute('aria-label') || 'Search'}"
                 aria-expanded="false"
                 aria-haspopup="listbox"
                 role="combobox" />
          <div class="ac-icon">
            ${icons.search('', 16)}
          </div>
        </div>
        <ul class="ac-dropdown hidden" role="listbox" aria-hidden="true"></ul>
      </div>
    `;

    this.#inputEl    = this.querySelector('.ac-input');
    this.#dropdownEl = this.querySelector('.ac-dropdown');
    this.#dd         = new DropdownController(this.#dropdownEl, this.#inputEl, '.ac-option');
  }

  set filterFn(fn) { this.#filterFn = fn; }

  #resolveFilterFn() {
    const name = this.getAttribute('filter-function');
    if (!name) {
      this.#filterFn = null;
      return;
    }
    if (typeof globalThis[name] === 'function') {
      this.#filterFn = globalThis[name];
    } else {
      this.#filterFn = null;
      // Module scripts execute in order; retry after current queue drains
      setTimeout(() => {
        if (typeof globalThis[name] === 'function') this.#filterFn = globalThis[name];
        else console.error('AutoComplete: filter function not found:', name);
      }, 0);
    }
  }

  #setupEvents() {
    this.#inputEl.addEventListener('input', () => this.#filterOptions());

    this.#inputEl.addEventListener('focus', async () => {
      if (!this.#filterFn) this.#resolveFilterFn();
      if (!this.#filterFn) return;
      try {
        this.#filteredOptions = await Promise.resolve(this.#filterFn(this.#inputEl.value.trim())) || [];
        this.#renderDropdown();
        if (this.#filteredOptions.length > 0) this.#dd.open();
      } catch (err) {
        console.error('AutoComplete: error on focus:', err);
      }
    });

    document.addEventListener('click', (e) => {
      if (!this.contains(e.target)) this.#dd.close();
    });

    this.#inputEl.addEventListener('keydown', (e) => {
      switch (e.key) {
        case 'ArrowDown': e.preventDefault(); this.#navigate(1);  break;
        case 'ArrowUp':   e.preventDefault(); this.#navigate(-1); break;
        case 'Enter':     e.preventDefault(); this.#selectOption(); break;
        case 'Escape':
          if (this.#dd.isOpen) { e.preventDefault(); this.#dd.close(); }
          break;
      }
    });
  }

  async #filterOptions() {
    if (!this.#filterFn) this.#resolveFilterFn();
    if (!this.#filterFn) { this.#filteredOptions = []; this.#dd.close(); return; }
    try {
      this.#filteredOptions = await Promise.resolve(this.#filterFn(this.#inputEl.value.trim())) || [];
      this.#renderDropdown();
      if (this.#filteredOptions.length > 0) this.#dd.open(); else this.#dd.close();
    } catch (err) {
      console.error('AutoComplete: error filtering:', err);
    }
  }

  #renderDropdown() {
    if (this.#filteredOptions.length === 0) {
      this.#dropdownEl.innerHTML = `
        <div class="ac-empty">
          ${icons.faceFrown('ac-empty-icon')}
          <span class="ac-empty-text">No results found</span>
        </div>
      `;
    } else {
      this.#dropdownEl.innerHTML = this.#filteredOptions
        .map((option, index) => {
          const label    = typeof option === 'string' ? option : option.label || option;
          const subtitle = typeof option === 'object' ? option.subtitle : null;
          return `
            <li class="ac-option" data-index="${index}" role="option" aria-selected="false">
              <div class="ac-option-body">
                <div class="ac-option-text">${this.#escapeHtml(label)}</div>
                ${subtitle ? `<div class="ac-option-subtitle">${this.#escapeHtml(subtitle)}</div>` : ''}
              </div>
            </li>
          `;
        })
        .join('');
    }
    this.#dd.bindItems(this.#filteredOptions.length, () => this.#selectOption());
  }

  async #navigate(dir) {
    if (!this.#filterFn) this.#resolveFilterFn();
    if (!this.#dd.isOpen) {
      if (!this.#filterFn) return;
      try {
        this.#filteredOptions = await Promise.resolve(this.#filterFn(this.#inputEl.value.trim())) || [];
        this.#renderDropdown();
        if (this.#filteredOptions.length > 0) this.#dd.open(); else return;
      } catch (err) {
        console.error('AutoComplete: error navigating:', err);
        return;
      }
    }
    this.#dd.navigate(dir);
  }

  #selectOption() {
    let idx = this.#dd.selIdx;
    if (idx === -1 && this.#filteredOptions.length > 0) idx = 0;
    if (idx < 0 || idx >= this.#filteredOptions.length) return;

    const option  = this.#filteredOptions[idx];
    const display = typeof option === 'string' ? option : option.label || option;
    const value   = typeof option === 'string' ? option : option.value || option;

    this.#inputEl.value = display;
    this.dispatchEvent(new CustomEvent('option-selected', { bubbles: true, detail: { value, option } }));
    this.#dd.close();
  }

  set value(v) { this.#inputEl.value = v; }
  get value()  { return this.#inputEl.value; }

  #escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }
}

customElements.define('auto-complete', AutoComplete);
