/**
 * Paginated, searchable data list backed by a `window[data-fn]` callback function.
 *
 * @element data-view
 * @attr {number} limit - Items per page (default: 10)
 * @attr {string} data-fn - Name of `window[data-fn](query, page, limit)` async function that returns `{items, total}`
 * @attr {string} group-fn - Name of `window[group-fn](items)` function for grouping rows
 * @attr {string} item-component - Custom element tag name used to render each row
 * @attr {string} search-placeholder - Placeholder text for the search input
 * @attr {string} empty-message - Message shown when there are no results
 * @attr {boolean} search - Show the search input (omit to hide)
 * @attr {string} columns - CSS grid-template-columns for the item layout
 * @method refresh() - Re-fetch data and re-render (preserves current page and query)
 * @fires loading-start - Fires before each fetch; `detail: { message }`
 * @fires loading-end - Fires after each fetch; `detail: { message }`
 * @note `window[data-fn]` is called with `(searchQuery, page, limit)` and must return `{ items, total }`.
 */
import { createEventTracker, debounce } from '../utils/data-component-utils.js';
import { icons } from '../utils/icons.js';
import './app-skeleton.js';
import styles from './data-view.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export class DataView extends HTMLElement {
  #data = [];
  #currentPage = 1;
  #totalItems = 0;
  #searchQuery = '';
  #events = createEventTracker();
  #initialized = false;
  #colMin = null;
  #dataFnName = null;
  #groupFnName = null;
  #debouncedSearch = null;

  limit = 10;
  dataFn = null;
  groupFn = null;
  itemComponent = null;
  searchPlaceholder = 'Search...';
  emptyMessage = 'No results found';
  showSearch = false;

  static get observedAttributes() {
    return ['limit', 'data-fn', 'item-component', 'search-placeholder', 'empty-message', 'search', 'columns', 'group-fn'];
  }

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.limit = parseInt(this.getAttribute('limit')) || 10;
    const fnName = this.getAttribute('data-fn');
    this.itemComponent = this.getAttribute('item-component');
    this.searchPlaceholder = this.getAttribute('search-placeholder') || 'Search...';
    this.emptyMessage = this.getAttribute('empty-message') || 'No results found';
    this.showSearch = this.hasAttribute('search');
    this.#colMin = this.getAttribute('columns');

    const groupFnName = this.getAttribute('group-fn');
    if (groupFnName) {
      this.#groupFnName = groupFnName;
      this.groupFn = window[groupFnName] || null;
    }

    if (!fnName && !this.dataFn) {
      console.error('data-view: data-fn attribute is required or dataFn property must be set');
      return;
    }

    if (!this.itemComponent) {
      console.error('data-view: item-component attribute is required');
      return;
    }

    if (fnName) {
      this.#dataFnName = fnName;
      this.dataFn = window[fnName] || null;
    }

    this.render();
    this.#bindSearchListener();
    this.#bindPageListeners();

    if (this.#colMin) {
      this.style.setProperty('--pl-col-min', `${this.#colMin}px`);
      const container = this.querySelector('.items');
      if (container) container.classList.add('is-grid');
    }

    this.refresh();
  }

  disconnectedCallback() {
    if (this.#debouncedSearch) this.#debouncedSearch.cancel();
    this.#events.cleanup();
  }

  render() {
    this.innerHTML = `
      ${this.showSearch ? `
        <input
          type="search"
          class="search"
          placeholder="${this.searchPlaceholder}"
          value="${this.#searchQuery}"
          aria-label="Search items"
        >
      ` : ''}

      <div class="items" role="list" aria-live="polite" aria-atomic="false"></div>

        <div class="pagination" role="navigation" aria-label="Pagination" hidden>
          <button class="page-btn is-prev" aria-label="Previous page">
            ${icons.chevronLeft('', 16)}
            <span>Prev</span>
            <kbd>[</kbd>
          </button>

          <span class="page-info"></span>

          <button class="page-btn is-next" aria-label="Next page">
            <kbd>]</kbd>
            <span>Next</span>
            ${icons.chevronRight('', 16)}
          </button>
        </div>

      <div class="error" role="alert" hidden></div>
    `;
  }

  #setupSearchInput(input) {
    if (this.#debouncedSearch) {
      this.#debouncedSearch.cancel();
    }
    this.#debouncedSearch = debounce(() => {
      this.#searchQuery = input.value;
      this.#currentPage = 1;
      this.refresh();
    }, 300);
    this.#events.add(input, 'input', this.#debouncedSearch);
  }

  #bindSearchListener() {
    if (!this.showSearch) return;
    const searchInput = this.querySelector('.search');
    if (searchInput) {
      this.#setupSearchInput(searchInput);
    }
  }

  #bindPageListeners() {
    const prevButton = this.querySelector('.page-btn.is-prev');
    const nextButton = this.querySelector('.page-btn.is-next');
    if (prevButton) this.#events.add(prevButton, 'click', () => this.changePage(-1));
    if (nextButton) this.#events.add(nextButton, 'click', () => this.changePage(1));
  }

  #toggleSearch(enable) {
    const existing = this.querySelector('.search');
    if (enable && !existing) {
      const input = document.createElement('input');
      input.type = 'search';
      input.className = 'search';
      input.placeholder = this.searchPlaceholder;
      input.value = this.#searchQuery;
      input.setAttribute('aria-label', 'Search items');
      this.insertBefore(input, this.querySelector('.items'));
      this.#setupSearchInput(input);
    } else if (!enable && existing) {
      if (this.#debouncedSearch) this.#debouncedSearch.cancel();
      existing.remove();
      this.#searchQuery = '';
    }
  }

  async refresh() {
    if (!this.dataFn && this.#dataFnName) {
      this.dataFn = window[this.#dataFnName] || null;
    }
    if (!this.groupFn && this.#groupFnName) {
      this.groupFn = window[this.#groupFnName] || null;
    }
    if (!this.dataFn) return;

    this.hideError();
    this.#showSkeletons();

    this.dispatchEvent(new CustomEvent('loading-start', {
      bubbles: true,
      detail: { message: 'Loading data...' }
    }));

    try {
      const response = await this.dataFn(this.#searchQuery, this.#currentPage, this.limit);
      if (Array.isArray(response)) {
        this.#data = response;
        this.#totalItems = response.length;
      } else {
        this.#data = response.data || [];
        this.#totalItems = response.total ?? this.#data.length;
      }

      this.renderItems();
      this.updatePagination();
    } catch (error) {
      console.error('Error fetching data:', error);
      this.showError('Failed to load data. Please try again.');
    } finally {
      this.dispatchEvent(new CustomEvent('loading-end', {
        bubbles: true,
        detail: { message: 'Data loaded' }
      }));
    }
  }

  #showSkeletons() {
    const container = this.querySelector('.items');
    if (!container) return;
    const count = Math.min(this.limit, 8);
    container.innerHTML = Array.from({ length: count }, () => `
      <div class="pl-skeleton-card" aria-hidden="true">
        <app-skeleton lines="3"></app-skeleton>
        <app-skeleton lines="1"></app-skeleton>
      </div>
    `).join('');
    // Disable page buttons while loading so the user can't double-navigate
    this.querySelector('.page-btn.is-prev')?.setAttribute('disabled', '');
    this.querySelector('.page-btn.is-next')?.setAttribute('disabled', '');
  }

  renderItems() {
    const container = this.querySelector('.items');
    if (!container) return;

    if (!this.#data || this.#data.length === 0) {
      container.innerHTML = `<div class="empty">${this.emptyMessage}</div>`;
      return;
    }

    container.innerHTML = '';
    let lastGroup;

    for (const item of this.#data) {
      if (this.groupFn) {
        const group = this.groupFn(item);
        if (group !== lastGroup) {
          const header = document.createElement('div');
          header.className = 'group-header';
          header.setAttribute('aria-hidden', 'true');
          header.textContent = group;
          container.appendChild(header);
          lastGroup = group;
        }
      }

      const itemElement = document.createElement(this.itemComponent);
      itemElement.setAttribute('role', 'listitem');
      itemElement.itemData = item;

      if (typeof item === 'object' && item !== null) {
        for (const [key, val] of Object.entries(item)) {
          if (val !== null && val !== undefined) {
            itemElement.setAttribute(`data-${key}`, String(val));
          }
        }
      }

      container.appendChild(itemElement);
    }
  }

  updatePagination() {
    const pagination = this.querySelector('.pagination');
    const prevButton = this.querySelector('.page-btn.is-prev');
    const nextButton = this.querySelector('.page-btn.is-next');
    const pageInfo = this.querySelector('.page-info');

    if (!pagination || !prevButton || !nextButton || !pageInfo) return;

    const totalPages = Math.ceil(this.#totalItems / this.limit);

    if (totalPages <= 1) {
      pagination.hidden = true;
      return;
    }

    pagination.hidden = false;

    const isPrevDisabled = this.#currentPage === 1;
    const isNextDisabled = this.#currentPage >= totalPages;

    prevButton.disabled = isPrevDisabled;
    nextButton.disabled = isNextDisabled;
    prevButton.setAttribute('aria-disabled', isPrevDisabled.toString());
    nextButton.setAttribute('aria-disabled', isNextDisabled.toString());

    const startItem = (this.#currentPage - 1) * this.limit + 1;
    const endItem = Math.min(this.#currentPage * this.limit, this.#totalItems);
    pageInfo.textContent = `${startItem}–${endItem} of ${this.#totalItems}`;
  }

  changePage(delta) {
    const totalPages = Math.ceil(this.#totalItems / this.limit);
    const newPage = this.#currentPage + delta;

    if (newPage < 1 || newPage > totalPages) return;

    this.#currentPage = newPage;
    this.refresh();
    this.scrollIntoView({ block: 'nearest' });
  }

  showError(message) {
    const errorElement = this.querySelector('.error');
    if (errorElement) {
      errorElement.textContent = message;
      errorElement.hidden = false;
    }
    const container = this.querySelector('.items');
    if (container) {
      container.innerHTML = '';
    }
    const pagination = this.querySelector('.pagination');
    if (pagination) {
      pagination.hidden = true;
    }
  }

  hideError() {
    const errorElement = this.querySelector('.error');
    if (errorElement) {
      errorElement.hidden = true;
    }
  }

  attributeChangedCallback(name, oldValue, newValue) {
    if (oldValue === newValue || !this.isConnected) return;

    switch (name) {
      case 'limit':
        this.limit = parseInt(newValue) || 10;
        this.#currentPage = 1;
        this.refresh();
        break;
      case 'data-fn':
        this.#dataFnName = newValue;
        this.dataFn = window[newValue] || null;
        this.#currentPage = 1;
        this.refresh();
        break;
      case 'item-component':
        this.itemComponent = newValue;
        this.renderItems();
        break;
      case 'search-placeholder':
        this.searchPlaceholder = newValue;
        const searchInput = this.querySelector('.search');
        if (searchInput) searchInput.placeholder = newValue;
        break;
      case 'empty-message':
        this.emptyMessage = newValue || 'No results found';
        break;
      case 'search':
        this.showSearch = newValue !== null;
        this.#toggleSearch(this.showSearch);
        if (!this.showSearch) this.refresh();
        break;
      case 'columns':
        this.#colMin = newValue;
        const container = this.querySelector('.items');
        if (newValue) {
          this.style.setProperty('--pl-col-min', `${newValue}px`);
          if (container) container.classList.add('is-grid');
        } else {
          this.style.removeProperty('--pl-col-min');
          if (container) container.classList.remove('is-grid');
        }
        break;
      case 'group-fn':
        this.#groupFnName = newValue;
        this.groupFn = newValue ? (window[newValue] || null) : null;
        if (this.#initialized) this.renderItems();
        break;
    }
  }
}

customElements.define('data-view', DataView);
