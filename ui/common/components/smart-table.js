/**
 * Paginated data table with search, auto-detected columns, and an optional row detail panel.
 *
 * @element smart-table
 * @attr {number} limit - Rows per page (default: 10)
 * @attr {string} data-fn - Name of `window[data-fn](query, page, limit)` async function returning `{ items, total }`
 * @attr {string} search-placeholder - Placeholder text for the search input
 * @attr {boolean} search - Show the search input
 * @attr {string} detail - CSS selector or element name to render a detail panel on row click
 * @fires loading-start - Before each fetch — bubbles
 * @fires loading-end - After each fetch — bubbles
 * @note Columns are inferred from the first item's keys, or define `<col data-key="…" data-label="…">` children.
 */
import { icons } from '../utils/icons.js';
import '/common/components/app-modal.js';


import { createEventTracker, debounce } from '../utils/data-component-utils.js';
import styles from './smart-table.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

// Icon references — sourced from the shared icons library
const IC_SORT_BOTH = icons.sortBoth('sort-icon');
const IC_SORT_ASC  = icons.sortAsc('sort-icon is-active');
const IC_SORT_DESC = icons.sortDesc('sort-icon is-active');
const IC_PREV      = icons.pagePrev();
const IC_NEXT      = icons.pageNext();
const IC_SEARCH    = icons.search('', 14);

export class SmartTable extends HTMLElement {
  #data = [];
  #sortedData = [];
  #currentPage = 1;
  #sortField = null;
  #sortDirection = 'asc';
  #totalItems = 0;
  #searchQuery = '';
  #events = createEventTracker();
  #dataFnName = null;
  #debouncedSearch = null;
  #showDetail = false;

  /**
   * Optional column definitions. If set, controls which columns are shown,
   * their labels, and how cells are rendered.
   *
   * Each entry: { key, label?, width?, wrap?, render?(value, row) => string }
   *
   * - `key`    — the property name on each data row
   * - `label`  — column header text (defaults to key)
   * - `width`  — CSS width string applied via <colgroup> (e.g. '30%')
   * - `wrap`   — if true, cell content wraps instead of truncating
   * - `render` — returns an HTML string for the cell; raw value used if omitted
   */
  columns = null;

  constructor() {
    super();
    this.limit = 10;
    this.dataFn = null;
    this.searchPlaceholder = 'Search...';
    this.showSearch = false;
  }

  static get observedAttributes() {
    return ['limit', 'data-fn', 'search-placeholder', 'search', 'detail', 'empty-message'];
  }

  connectedCallback() {
    this.limit = parseInt(this.getAttribute('limit')) || 10;
    const fnName = this.getAttribute('data-fn');
    this.searchPlaceholder = this.getAttribute('search-placeholder') || 'Search...';
    this.showSearch = this.hasAttribute('search');
    this.#showDetail = this.hasAttribute('detail');

    if (fnName) {
      this.#dataFnName = fnName;
      this.dataFn = window[fnName] || null;
    }

    this.#render();
    this.#setupEventListeners();
    // Deferred by a microtask: pages assign `columns` on the statement after
    // the element is created, which is after this callback has run. Waiting
    // lets the skeleton pass draw the real header and column widths, so rows
    // never move between the loading state and loaded data.
    queueMicrotask(() => this.refresh());
  }

  disconnectedCallback() {
    if (this.#debouncedSearch) this.#debouncedSearch.cancel();
    this.#events.cleanup();
  }

  #render() {
    this.innerHTML = `
      <div class="smart-table">
        ${this.showSearch ? `
          <div class="search-wrap">
            <span class="search-icon">${IC_SEARCH}</span>
            <input
              type="search"
              class="search"
              placeholder="${this.#escapeAttr(this.searchPlaceholder)}"
              value="${this.#escapeAttr(this.#searchQuery)}"
              aria-label="Search table data"
            >
          </div>
        ` : ''}

        <div class="scroll">
          <table class="table">
            <thead class="thead"></thead>
            <tbody class="tbody"></tbody>
          </table>
        </div>

        <nav class="pagination" role="navigation" aria-label="Table pagination" hidden>
          <button class="page-btn is-prev" aria-label="Previous page">${IC_PREV}</button>
          <span class="page-info">Page ${this.#currentPage}</span>
          <button class="page-btn is-next" aria-label="Next page">${IC_NEXT}</button>
        </nav>

        <div class="error" role="alert" hidden></div>
      </div>

      ${this.#showDetail ? `
        <app-modal class="detail-modal" heading="Row Detail">
          <dl class="detail-list"></dl>
        </app-modal>
      ` : ''}
    `;
  }

  #setupEventListeners() {
    if (this.showSearch) {
      const searchInput = this.querySelector('input.search');
      if (searchInput) {
        this.#debouncedSearch = debounce(() => {
          this.#searchQuery = searchInput.value;
          this.#currentPage = 1;
          this.refresh();
        }, 300);
        this.#events.add(searchInput, 'input', this.#debouncedSearch);
      }
    }

    const prevBtn = this.querySelector('.page-btn.is-prev');
    const nextBtn = this.querySelector('.page-btn.is-next');
    if (prevBtn) this.#events.add(prevBtn, 'click', () => this.#changePage(-1));
    if (nextBtn) this.#events.add(nextBtn, 'click', () => this.#changePage(1));

    if (this.#showDetail) {
      const tbody = this.querySelector('.tbody');
      if (tbody) {
        this.#events.add(tbody, 'click', (e) => {
          if (e.target.closest('a, button, [data-action]')) return;
          const tr = e.target.closest('tr[data-row-index]');
          if (!tr) return;
          const idx = parseInt(tr.dataset.rowIndex, 10);
          const row = this.#getSortedData()[idx];
          if (row) this.#openDetail(row);
        });
      }
    }
  }

  async refresh() {
    if (!this.dataFn && this.#dataFnName) {
      this.dataFn = window[this.#dataFnName] || null;
    }
    if (!this.dataFn) return;

    this.#hideError();
    this.#showSkeletons();

    const scroll = this.querySelector('.scroll');
    if (scroll) scroll.classList.add('is-loading');

    this.dispatchEvent(new CustomEvent('loading-start', { bubbles: true, detail: { message: 'Loading data...' } }));

    try {
      const response = await this.dataFn(this.#searchQuery, this.#currentPage, this.limit);
      // A fetcher may answer a bare array or a {data, total} envelope. Anything
      // else is a contract mismatch (e.g. an endpoint returning a differently
      // named key): degrade to an empty table with a pointed warning rather
      // than assigning a non-iterable and throwing deep in the sort.
      const rows = Array.isArray(response) ? response : response?.data;
      if (!Array.isArray(rows)) {
        console.warn(
          `smart-table: "${this.#dataFnName}" returned no row array (expected an array or {data:[…]}); got`,
          response,
        );
      }
      this.#data = Array.isArray(rows) ? rows : [];
      this.#totalItems = response?.total ?? this.#data.length;
      this.#invalidateSortCache();
      this.#renderTable();
      this.#updatePagination();
    } catch (error) {
      console.error('smart-table: Error fetching data:', error);
      this.#showError('Failed to load data. Please try again.');
    } finally {
      if (scroll) scroll.classList.remove('is-loading');
      this.dispatchEvent(new CustomEvent('loading-end', { bubbles: true, detail: { message: 'Data loaded' } }));
    }
  }

  /** Show skeleton rows while data loads — gives the table a stable size hint. */
  #showSkeletons() {
    const thead = this.querySelector('.thead');
    const tbody = this.querySelector('.tbody');
    if (!thead || !tbody) return;
    if (!this.#data.length) {
      this.#renderColgroup(this.columns);
      this.#renderHead(this.columns);
    }
    const colCount = this.columns ? this.columns.length : 4;
    const widthSets = [
      ['60%','80%','40%','70%'],
      ['75%','55%','65%','50%'],
      ['50%','90%','45%','80%'],
      ['70%','60%','80%','55%'],
      ['65%','75%','50%','70%'],
    ];
    const skeletonRow = (i) => {
      const ws = widthSets[i % widthSets.length];
      const cells = Array.from({ length: colCount }, (_, j) =>
        `<td class="td"><div style="width:${ws[j % ws.length]};height:0.75em;background:var(--color-border);border-radius:var(--radius-sm);"></div></td>`
      ).join('');
      return `<tr>${cells}</tr>`;
    };
    tbody.innerHTML = Array.from({ length: this.limit }, (_, i) => skeletonRow(i)).join('');
  }

  #renderTable() {
    const thead = this.querySelector('.thead');
    const tbody = this.querySelector('.tbody');
    if (!thead || !tbody) return;

    if (!this.#data || this.#data.length === 0) {
      // Keep the header row. Blanking it left a <colgroup> sizing columns that
      // had no headers above them, so an empty table read as a broken one.
      this.#renderColgroup(this.columns);
      this.#renderHead(this.columns);
      // "No results found" is only true when something was actually searched
      // for — on a table with no active query it told the user their own filter
      // came up empty on a filter they never set.
      const message = this.#searchQuery
        ? `No results for “${this.#escapeHtml(this.#searchQuery)}”`
        : (this.getAttribute('empty-message') || 'Nothing here yet');
      tbody.innerHTML = `<tr><td class="empty" colspan="100%">${message}</td></tr>`;
      return;
    }

    // Use explicit column definitions if provided, otherwise derive from data keys
    const cols = this.columns
      ? this.columns
      : Object.keys(this.#data[0]).map(k => ({ key: k, label: k }));

    // Rebuild colgroup on every render to stay consistent across sort/re-renders
    this.#renderColgroup(cols);
    this.#renderHead(cols);

    // Rebind sort events on headers
    this.#events.removeTagged('_header');
    thead.querySelectorAll('.th[data-field]').forEach(th => {
      const field = th.dataset.field;
      const clickHandler = () => this.#sort(field);
      const keyHandler = (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          this.#sort(field);
        }
      };
      this.#events.add(th, 'click', clickHandler, { _header: true });
      this.#events.add(th, 'keydown', keyHandler, { _header: true });
    });

    const displayData = this.#getSortedData();
    tbody.innerHTML = displayData.map((row, i) => `
      <tr${this.#showDetail ? ` class="is-clickable" data-row-index="${i}"` : ''}>${cols.map(col => {
        const raw = row[col.key];
        const cell = col.render
          ? col.render(raw, row)
          : `<span title="${this.#escapeAttr(raw)}">${this.#escapeHtml(raw)}</span>`;
        // `is-plain` mirrors the header marker for label-less (row-action)
        // columns, so CSS can pin the action cell and its header together.
        const plain = !String(col.label ?? col.key).trim() ? ' is-plain' : '';
        return `<td class="td${col.wrap ? ' is-wrap' : ''}${plain}">${cell}</td>`;
      }).join('')}</tr>
    `).join('');
  }

  #renderColgroup(cols) {
    const table = this.querySelector('.table');
    if (!table) return;
    table.querySelector('colgroup')?.remove();
    if (!cols || !cols.some(c => c.width)) return;
    const cg = document.createElement('colgroup');
    cg.innerHTML = cols.map(c => `<col${c.width ? ` style="width:${c.width}"` : ''}>`).join('');
    table.prepend(cg);
  }

  /** Header row — shared by the skeleton pass and the data render, so the
   *  first body row sits at the same y in both. */
  #renderHead(cols) {
    const thead = this.querySelector('.thead');
    if (!thead) return;
    if (!cols) {
      thead.innerHTML = '';
      return;
    }
    thead.innerHTML = `<tr>${cols.map(col => {
      const field = col.key;
      const label = col.label ?? field;
      // Label-less columns (row actions) get a plain, non-sortable header —
      // a focusable empty "Sort by" control is noise for everyone.
      if (!String(label).trim()) {
        return `<th class="th is-plain" role="columnheader"></th>`;
      }
      let icon = IC_SORT_BOTH;
      let ariaSort = 'none';
      if (this.#sortField === field) {
        icon = this.#sortDirection === 'asc' ? IC_SORT_ASC : IC_SORT_DESC;
        ariaSort = this.#sortDirection === 'asc' ? 'ascending' : 'descending';
      }
      return `
        <th class="th"
            data-field="${this.#escapeAttr(field)}"
            tabindex="0"
            role="columnheader"
            aria-sort="${ariaSort}"
            aria-label="Sort by ${this.#escapeHtml(label)}">
          <div class="th-content">
            <span>${this.#escapeHtml(label)}</span>
            ${icon}
          </div>
        </th>`;
    }).join('')}</tr>`;
  }

  #updatePagination() {
    const pagination = this.querySelector('.pagination');
    const prevBtn = this.querySelector('.page-btn.is-prev');
    const nextBtn = this.querySelector('.page-btn.is-next');
    const pageInfo = this.querySelector('.page-info');
    if (!pagination || !prevBtn || !nextBtn || !pageInfo) return;

    const totalPages = Math.ceil(this.#totalItems / this.limit);

    if (totalPages <= 1) {
      pagination.hidden = true;
      return;
    }

    pagination.hidden = false;
    const isPrevDisabled = this.#currentPage === 1;
    const isNextDisabled = this.#currentPage >= totalPages;

    prevBtn.disabled = isPrevDisabled;
    nextBtn.disabled = isNextDisabled;
    prevBtn.setAttribute('aria-disabled', isPrevDisabled.toString());
    nextBtn.setAttribute('aria-disabled', isNextDisabled.toString());

    pageInfo.textContent = `Page ${this.#currentPage} of ${totalPages}`;
  }

  #sort(field) {
    if (this.#sortField === field) {
      this.#sortDirection = this.#sortDirection === 'asc' ? 'desc' : 'asc';
    } else {
      this.#sortField = field;
      this.#sortDirection = 'asc';
    }
    this.#invalidateSortCache();
    this.#renderTable();
  }

  #changePage(delta) {
    const totalPages = Math.ceil(this.#totalItems / this.limit);
    const newPage = this.#currentPage + delta;
    if (newPage < 1 || newPage > totalPages) return;
    this.#currentPage = newPage;
    this.refresh();
  }

  #getSortedData() {
    if (!this.#sortField || !this.#data.length) return [...this.#data];
    if (this.#sortedData.length > 0) return this.#sortedData;

    this.#sortedData = [...this.#data].sort((a, b) => {
      const aVal = a[this.#sortField];
      const bVal = b[this.#sortField];
      const dir = this.#sortDirection === 'asc' ? 1 : -1;

      if (typeof aVal === 'number' && typeof bVal === 'number') {
        return (aVal - bVal) * dir;
      }
      return String(aVal).localeCompare(String(bVal)) * dir;
    });
    return this.#sortedData;
  }

  #invalidateSortCache() {
    this.#sortedData = [];
  }

  #escapeHtml(value) {
    if (value == null) return '';
    return String(value)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  #escapeAttr(value) {
    if (value == null) return '';
    return String(value)
      .replace(/&/g, '&amp;')
      .replace(/"/g, '&quot;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;');
  }

  #openDetail(row) {
    const modal = this.querySelector('.detail-modal');
    if (!modal) return;
    const dl = modal.querySelector('.detail-list');
    if (!dl) return;
    const cols = this.columns
      ? this.columns
      : Object.keys(row).map(k => ({ key: k, label: k }));
    dl.innerHTML = cols.map(col => {
      const raw = row[col.key];
      const val = raw == null ? '' : String(raw);
      return `
        <div class="detail-item">
          <dt class="detail-key">${this.#escapeHtml(col.label ?? col.key)}</dt>
          <dd class="detail-val">${this.#escapeHtml(val)}</dd>
        </div>`;
    }).join('');
    modal.open();
  }

  #showError(message) {
    const el = this.querySelector('.error');
    if (el) {
      el.textContent = message;
      el.hidden = false;
    }
  }

  #hideError() {
    const el = this.querySelector('.error');
    if (el) el.hidden = true;
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
      case 'search-placeholder':
        this.searchPlaceholder = newValue;
        const input = this.querySelector('.search');
        if (input) input.placeholder = newValue;
        break;
      case 'search':
        this.showSearch = newValue !== null;
        this.#render();
        this.#setupEventListeners();
        this.refresh();
        break;
      case 'detail':
        this.#showDetail = newValue !== null;
        this.#render();
        this.#setupEventListeners();
        this.refresh();
        break;
    }
  }
}

customElements.define('smart-table', SmartTable);
