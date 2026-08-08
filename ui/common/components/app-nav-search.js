/**
 * Global ⌘F command palette used inside `<app-header>`: searches pages plus
 * live control-plane data — agents (`window.fetchAgents`), MCP connectors
 * (`window.fetchMcpConnectors`), and recent chat sessions
 * (`window.fetchSessions`) — in labeled groups. Data loads once per open and
 * filters client-side per keystroke; sources that fail (or don't exist on a
 * deployment) simply omit their group.
 *
 * @element app-nav-search
 * @fires navigate - Item selected; `detail: { url, newTab }` — bubbles
 */
import { icons } from "../utils/icons.js";
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-nav-search) {
    .nav-dialog {
      border-radius: clamp(0px, 3vw, var(--radius-lg));
      max-width: min(600px, 90vw);
      width: min(100% - var(--space-xl), 600px);
      margin: clamp(0rem, 5vh, var(--space-2xl)) auto;
      overflow: hidden;
      &::backdrop { background: rgba(0, 0, 0, 0.5); }
    }
    .nav-search { display: flex; flex-direction: column; }
    .input-row {
      display: flex;
      align-items: center;
      gap: var(--space-sm);
      padding: var(--space-lg) var(--space-xl);
      border-bottom: 1px solid var(--color-border);
    }
    .input-icon { flex-shrink: 0; width: 18px; height: 18px; color: var(--color-text-muted); }
    .input {
      flex: 1;
      border: none;
      background: transparent;
      color: var(--color-text-main);
      font-size: var(--font-size-base);
      padding: 0;
      min-width: 0;
      &:focus { outline: none; }
      &::placeholder { color: var(--color-text-muted); }
    }
    .esc-hint {
      flex-shrink: 0;
      font-size: var(--font-size-xs);
      color: var(--color-text-muted);
      background: var(--color-bg-base);
      border: 1px solid var(--color-border);
      border-radius: var(--radius-sm);
      padding: 2px var(--space-xs);
      cursor: default;
      user-select: none;
    }
    .results {
      list-style: none;
      padding: var(--space-xs) 0;
      margin: 0;
      max-height: min(400px, 60vh);
      overflow-y: auto;
      & .group-head {
        padding: var(--space-sm) var(--space-lg) var(--space-2xs);
        font-size: var(--font-size-xs);
        font-weight: 600;
        letter-spacing: 0.4px;
        text-transform: uppercase;
        color: var(--color-text-muted);
        user-select: none;
      }
      &::-webkit-scrollbar { width: 6px; }
      &::-webkit-scrollbar-track { background: transparent; }
      &::-webkit-scrollbar-thumb { background: var(--color-border); border-radius: 3px; }
    }
    .result {
      display: flex;
      align-items: center;
      gap: var(--space-sm);
      padding: var(--space-sm) var(--space-lg);
      cursor: pointer;
      border-left: 3px solid transparent;
      &:hover { background: var(--color-bg-base); }
      &.is-active {
        background: var(--color-bg-base);
        border-left-color: var(--color-primary);
        & .result-label { color: var(--color-primary); font-weight: 600; }
        & .result-icon { color: var(--color-primary); }
        & .result-arrow { opacity: 1; color: var(--color-primary); }
      }
      &.is-current { & .result-label { font-weight: 600; } }
    }
    .result-icon { flex-shrink: 0; width: 16px; height: 16px; color: var(--color-text-muted); }
    .result-body { flex: 1; min-width: 0; }
    .result-label { font-size: var(--font-size-sm); font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .result-subtitle { font-size: var(--font-size-xs); color: var(--color-text-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; margin-top: 1px; }
    .result-arrow { flex-shrink: 0; width: 14px; height: 14px; color: var(--color-text-muted); opacity: 0; }
    .result-current-dot { display: block; width: 6px; height: 6px; border-radius: 50%; background: var(--color-primary); flex-shrink: 0; }
    .empty {
      display: flex;
      flex-direction: column;
      align-items: center;
      padding: var(--space-2xl) var(--space-lg);
      color: var(--color-text-muted);
      gap: var(--space-sm);
    }
    .empty-icon { width: 32px; height: 32px; opacity: 0.4; }
    .empty-text { font-size: var(--font-size-sm); }
    .footer {
      display: flex;
      align-items: center;
      gap: var(--space-lg);
      padding: var(--space-sm) var(--space-lg);
      border-top: 1px solid var(--color-border);
      background: var(--color-bg-base);
    }
    .footer-hint { display: flex; align-items: center; gap: var(--space-xs); font-size: var(--font-size-xs); color: var(--color-text-muted); }
    .footer-key {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      font-size: var(--font-size-xs);
      color: var(--color-text-muted);
      background: var(--color-bg-surface);
      border: 1px solid var(--color-border);
      border-radius: var(--radius-sm);
      padding: 1px 5px;
    }
    .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border-width: 0; }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];



/** Global search dialog for app-header. Emits `navigate` event. */
export class AppNavSearch extends HTMLElement {
  #navLinks = [];
  #userPrefix = '';
  #isMac = /Mac|iPhone|iPad|iPod/i.test(navigator.userAgent);
  #results = [];
  #selectedIndex = -1;
  #dialog;
  #input;
  #resultsList;
  // Live control-plane data, loaded once per open (small lists; filtered
  // client-side per keystroke). Missing/failed sources stay empty arrays.
  #agents = [];
  #connectors = [];
  #sessions = [];
  #loadToken = 0;

  set navLinks(v) { this.#navLinks = v; }
  set userPrefix(v) { this.#userPrefix = v; }

  connectedCallback() {
    this.innerHTML = `
      <dialog class="nav-dialog" data-nav-dialog aria-label="Navigation search">
        <div class="nav-search">
          <div class="input-row">
            ${icons.search('input-icon')}
            <input class="input" type="text" autocomplete="off"
              placeholder="Search pages…" data-nav-input aria-label="Search navigation"
              role="combobox" aria-autocomplete="list" aria-haspopup="listbox"/>
            <kbd class="esc-hint">ESC</kbd>
          </div>
          <ul class="results" data-nav-results role="listbox" aria-label="Search results"></ul>
          <div class="footer">
            <span class="footer-hint">
              <kbd class="footer-key">&#8595;</kbd>
              <kbd class="footer-key">&#8593;</kbd>
              navigate
            </span>
            <span class="footer-hint">
              <kbd class="footer-key">Enter</kbd> open
            </span>
            <span class="footer-hint">
              <kbd class="footer-key">${this.#isMac ? 'Cmd' : 'Ctrl'}</kbd>
              <kbd class="footer-key">Enter</kbd>
              new tab
            </span>
          </div>
        </div>
      </dialog>
    `;

    this.#dialog = this.querySelector('[data-nav-dialog]');
    this.#input = this.querySelector('[data-nav-input]');
    this.#resultsList = this.querySelector('[data-nav-results]');

    this.#input?.addEventListener('input', () => {
      this.#results = this.#filter(this.#input.value.trim());
      this.#selectedIndex = this.#results.length > 0 ? 0 : -1;
      this.#renderResults();
    });

    this.#input?.addEventListener('keydown', e => {
      switch (e.key) {
        case 'ArrowDown': e.preventDefault(); this.#move(1); break;
        case 'ArrowUp':   e.preventDefault(); this.#move(-1); break;
        case 'Enter':     e.preventDefault(); this.#confirm(e.ctrlKey || e.metaKey); break;
        case 'Escape':    e.preventDefault(); this.close(); break;
      }
    });

    this.#dialog?.addEventListener('cancel', e => { e.preventDefault(); this.close(); });
    this.#dialog?.addEventListener('click', e => { if (e.target === this.#dialog) this.close(); });
  }

  open() {
    if (!this.#dialog) return;
    if (this.#input) this.#input.placeholder = 'Search pages, agents, MCPs, chats…';
    this.#results = this.#filter('');
    this.#selectedIndex = this.#results.length > 0 ? 0 : -1;
    this.#renderResults();
    this.#dialog.showModal();
    setTimeout(() => this.#input?.focus(), 50);
    this.#loadSources();
  }

  /** Fetch agents / MCP connectors / recent chats; re-filter when they land. */
  async #loadSources() {
    const token = ++this.#loadToken;
    const settle = (p, apply) => p.then(apply).catch(() => { /* source unavailable */ });
    await Promise.all([
      typeof window.fetchAgents === 'function'
        ? settle(window.fetchAgents('', 1, 50), (r) => {
            this.#agents = Array.isArray(r) ? r : r?.data || [];
          })
        : null,
      typeof window.fetchMcpConnectors === 'function'
        ? settle(window.fetchMcpConnectors(), (r) => {
            const d = r?.data ?? r ?? {};
            this.#connectors = [...(d.created_by_you || []), ...(d.shared_with_you || [])];
          })
        : null,
      typeof window.fetchSessions === 'function'
        ? settle(window.fetchSessions('', 1, 25), (r) => {
            this.#sessions = Array.isArray(r) ? r : r?.data || [];
          })
        : null,
    ]);
    // Stale response of a previous open, or the dialog closed meanwhile.
    if (token !== this.#loadToken || !this.#dialog?.open) return;
    this.#results = this.#filter(this.#input?.value.trim() || '');
    if (this.#selectedIndex === -1 && this.#results.length) this.#selectedIndex = 0;
    this.#renderResults();
  }

  close() {
    if (!this.#dialog) return;
    this.#dialog.close();
    if (this.#input) this.#input.value = '';
    this.#results = [];
    this.#selectedIndex = -1;
  }

  #filter(query) {
    const q = query.toLowerCase();
    const matches = (...fields) =>
      !q || fields.some((f) => (f || '').toLowerCase().includes(q));

    const pages = this.#navLinks
      .filter((l) => matches(l.title, l.url, l.description))
      .map((l) => ({
        group: 'Pages', icon: 'document',
        label: l.title,
        value: this.#userPrefix + l.url,
        subtitle: l.description || l.url,
      }));

    const agents = this.#agents
      .filter((a) => matches(a.display_name, a.name, a.description))
      .slice(0, q ? 6 : 4)
      .map((a) => ({
        group: 'Agents', icon: 'bot',
        label: a.display_name || a.name,
        value: `${this.#userPrefix}/agent-card.html?id=${encodeURIComponent(a.id)}`,
        subtitle: (a.description || a.name || '').slice(0, 90),
      }));

    const connectors = this.#connectors
      .filter((c) => matches(c.display_name, c.name, c.url))
      .slice(0, q ? 6 : 3)
      .map((c) => ({
        group: 'MCP connectors', icon: 'server',
        label: c.display_name || c.name,
        value: `${this.#userPrefix}/mcp.html`,
        subtitle: c.url || c.name,
      }));

    const sessions = this.#sessions
      .filter((s) => matches(s.agent_name, s.last_message, s.title))
      .slice(0, q ? 6 : 4)
      .map((s) => ({
        group: 'Recent chats', icon: 'history',
        label: (s.title || s.last_message || '').slice(0, 70) || s.session_id,
        value: `${this.#userPrefix}/chat.html?session_id=${encodeURIComponent(s.session_id)}`
          + `&agent_id=${encodeURIComponent(s.agent_id || '')}`
          + `&agent_name=${encodeURIComponent(s.agent_name || 'Orchestrator')}`,
        subtitle: s.agent_name || 'Orchestrator',
      }));

    return [...pages, ...agents, ...connectors, ...sessions];
  }

  #normalize(p) {
    return p.replace(/\/index\.html$/, '/').replace(/\/$/, '') || '/';
  }

  #renderResults() {
    if (!this.#resultsList) return;

    if (this.#results.length === 0) {
      this.#resultsList.innerHTML = `
        <li class="empty">
          ${icons.faceFrown('empty-icon')}
          <span class="empty-text">No matches found</span>
        </li>`;
      return;
    }

    const currentPath = this.#normalize(window.location.pathname);
    let lastGroup = null;
    this.#resultsList.innerHTML = this.#results.map((item, i) => {
      const isCurrent = this.#normalize(item.value) === currentPath;
      const indicator = isCurrent
        ? `<span class="result-current-dot" aria-hidden="true"></span>
           <span class="sr-only">(current page)</span>`
        : `${icons.chevronRight('result-arrow')}`;
      const header = item.group && item.group !== lastGroup
        ? `<li class="group-head" role="presentation">${this.#esc(item.group)}</li>`
        : '';
      lastGroup = item.group;
      const iconHtml = (icons[item.icon] || icons.document)('result-icon');

      return `${header}
        <li class="result${i === this.#selectedIndex ? ' is-active' : ''}${isCurrent ? ' is-current' : ''}"
          data-idx="${i}" role="option" aria-selected="${i === this.#selectedIndex}">
          ${iconHtml}
          <div class="result-body">
            <div class="result-label">${this.#esc(item.label)}</div>
            ${item.subtitle ? `<div class="result-subtitle">${this.#esc(item.subtitle)}</div>` : ''}
          </div>
          ${indicator}
        </li>`;
    }).join('');

    this.#resultsList.querySelectorAll('[data-idx]').forEach(el => {
      el.addEventListener('click', e => {
        this.#selectedIndex = parseInt(el.getAttribute('data-idx'));
        this.#confirm(e.ctrlKey || e.metaKey);
      });
      el.addEventListener('mouseover', () => {
        const idx = parseInt(el.getAttribute('data-idx'));
        if (this.#selectedIndex !== idx) {
          this.#selectedIndex = idx;
          this.#updateHighlight();
        }
      });
    });
  }

  #updateHighlight() {
    this.#resultsList?.querySelectorAll('[data-idx]').forEach((el, i) => {
      const active = i === this.#selectedIndex;
      el.classList.toggle('is-active', active);
      el.setAttribute('aria-selected', String(active));
    });
  }

  #move(dir) {
    if (!this.#results.length) return;
    this.#selectedIndex = (this.#selectedIndex + dir + this.#results.length) % this.#results.length;
    this.#updateHighlight();
    this.#resultsList?.querySelector('.is-active')?.scrollIntoView({ block: 'nearest' });
  }

  #confirm(newTab = false) {
    if (this.#selectedIndex === -1 && this.#results.length > 0) this.#selectedIndex = 0;
    const item = this.#results[this.#selectedIndex];
    if (!item?.value) return;
    this.close();
    this.dispatchEvent(new CustomEvent('navigate', { bubbles: true, detail: { url: item.value, newTab } }));
  }

  #esc(text) {
    const d = document.createElement('div');
    d.textContent = text;
    return d.innerHTML;
  }
}

customElements.define('app-nav-search', AppNavSearch);
