/**
 * Global ⌘F search palette used inside `<app-header>`: a dark ink dropdown
 * anchored under the topbar search field (NightOwl "Global search panel").
 * Searches pages plus live control-plane entities — agents, workflows, MAF
 * executions, chat sessions, MCP connectors, Composio toolkits, builds, and
 * (EE only, feature-detected) users. Data loads once per open, in parallel,
 * rendering incrementally as each source lands; keystrokes filter
 * client-side after a short debounce. Sources that fail (or don't exist on
 * a deployment) simply omit their section.
 *
 * @element app-nav-search
 * @fires navigate - Item selected; `detail: { url, newTab }` — bubbles
 */
import { icons } from "../utils/icons.js";
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-nav-search) {
    /* Ink panel per the NightOwl mockup: shell background, radius 8, deep
       drop shadow, no backdrop dim. Anchored under the topbar search field
       by inline styles from #position(); these rules are the un-anchored
       fallback (top-centered, e.g. if the topbar field is missing). */
    .nav-dialog {
      margin: calc(var(--shell-topbar-height) + var(--s-8)) auto auto;
      width: min(480px, calc(100vw - 16px));
      padding: 0;
      border: none;
      border-radius: var(--r-8);
      background: var(--shell-bg);
      color: var(--shell-fg);
      box-shadow: 0 12px 32px rgba(0, 0, 0, 0.32);
      overflow: hidden;
      /* Mockup: no dim, no blur — outside clicks still close via the
         (invisible) backdrop. Overrides global dialog::backdrop. */
      &::backdrop { background: transparent; backdrop-filter: none; }
    }
    .nav-search { display: flex; flex-direction: column; }
    /* Input styled like the topbar search field it drops down from. */
    .input-row {
      display: flex;
      align-items: center;
      gap: var(--s-8);
      margin: var(--s-12) var(--s-12) var(--s-8);
      height: var(--control-h-sm);
      padding: 0 var(--s-12);
      border-radius: var(--r-6);
      background: var(--shell-control);
    }
    .input-icon { flex-shrink: 0; width: 15px; height: 15px; color: var(--shell-fg-muted); }
    .input {
      flex: 1;
      border: none;
      background: transparent;
      color: var(--shell-fg);
      font-size: 13px;
      padding: 0;
      min-width: 0;
      &:focus { outline: none; box-shadow: none; }
      &::placeholder { color: var(--shell-fg-muted); }
    }
    .esc-hint {
      flex-shrink: 0;
      font-family: inherit;
      font-size: 11px;
      color: var(--shell-fg-muted);
      background: transparent; /* global kbd rule paints a light chip */
      border: 1px solid rgba(255, 255, 255, 0.16);
      border-radius: var(--r-4);
      padding: 1px 5px;
      cursor: default;
      user-select: none;
    }
    .results {
      list-style: none;
      padding: 0 var(--s-8) var(--s-8);
      margin: 0;
      max-height: min(480px, calc(100dvh - 160px));
      overflow-y: auto;
      & .group-head {
        padding: var(--s-8) var(--s-8) var(--s-4);
        font-size: 11px;
        font-weight: 600;
        letter-spacing: 0.5px;
        text-transform: uppercase;
        color: var(--shell-fg-muted);
        user-select: none;
      }
      & .group-more {
        padding: 2px var(--s-8) var(--s-4);
        font-size: 12px;
        color: var(--shell-fg-muted);
        user-select: none;
      }
      &::-webkit-scrollbar { width: 6px; }
      &::-webkit-scrollbar-track { background: transparent; }
      &::-webkit-scrollbar-thumb { background: rgba(255, 255, 255, 0.18); border-radius: 3px; }
    }
    .result {
      display: flex;
      align-items: center;
      gap: var(--s-8);
      padding: 6px var(--s-8);
      border-radius: var(--r-6);
      cursor: pointer;
      &:hover { background: rgba(255, 255, 255, 0.06); }
      &.is-active {
        background: var(--shell-control-hover);
        & .result-icon { color: var(--shell-selected); }
        & .result-arrow { opacity: 1; color: var(--shell-selected); }
      }
      &.is-current { & .result-label { font-weight: 600; } }
    }
    .result-icon { flex-shrink: 0; width: 15px; height: 15px; color: var(--shell-fg-muted); }
    .result-body { flex: 1; min-width: 0; }
    .result-label { font-size: 13px; font-weight: 500; color: var(--shell-fg); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .result-subtitle { font-size: 12px; color: var(--shell-fg-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; margin-top: 1px; }
    .result-arrow { flex-shrink: 0; width: 13px; height: 13px; color: var(--shell-fg-muted); opacity: 0; }
    .result-current-dot { display: block; width: 6px; height: 6px; border-radius: 50%; background: var(--shell-selected); flex-shrink: 0; }
    .empty {
      display: flex;
      flex-direction: column;
      align-items: center;
      padding: var(--s-24) var(--s-12);
      color: var(--shell-fg-muted);
      gap: var(--s-8);
    }
    .empty-icon { width: 28px; height: 28px; opacity: 0.4; }
    .empty-text { font-size: 13px; }
    .footer {
      display: flex;
      align-items: center;
      gap: var(--s-12);
      padding: var(--s-8) var(--s-12);
      border-top: 1px solid rgba(255, 255, 255, 0.08);
    }
    .footer-hint { display: flex; align-items: center; gap: var(--s-4); font-size: 11px; color: var(--shell-fg-muted); }
    .footer-key {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      font-family: inherit;
      font-size: 11px;
      color: var(--shell-fg-muted);
      background: transparent; /* global kbd rule paints a light chip */
      border: 1px solid rgba(255, 255, 255, 0.16);
      border-radius: var(--r-4);
      padding: 0 4px;
    }
    .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border-width: 0; }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const FILTER_DEBOUNCE_MS = 150;

/** Unwraps the common list envelopes: bare array or {data:[...]}. */
const rowsOf = (r) => (Array.isArray(r) ? r : r?.data) || [];

/** Global search palette for app-header. Emits `navigate` event. */
export class AppNavSearch extends HTMLElement {
  #navLinks = [];
  #userPrefix = '';
  #anchor = null;
  #isMac = /Mac|iPhone|iPad|iPod/i.test(navigator.userAgent);
  #sections = [];
  #flat = [];
  #selectedIndex = -1;
  #dialog;
  #input;
  #resultsList;
  #debounceTimer;
  // Live control-plane data, loaded once per open (small lists; filtered
  // client-side per keystroke). Missing/failed sources stay empty arrays.
  #data = {
    agents: [], workflows: [], executions: [], sessions: [],
    connectors: [], toolkits: [], builds: [], users: [],
  };
  #usersUnavailable = false; // EE-only source; 404 on OSS hides the section
  #loadToken = 0;

  set navLinks(v) { this.#navLinks = v; }
  set userPrefix(v) { this.#userPrefix = v; }
  /** Topbar search field the panel anchors under (set by app-header). */
  set anchorEl(el) { this.#anchor = el; }

  connectedCallback() {
    this.innerHTML = `
      <dialog class="nav-dialog" data-nav-dialog aria-label="Global search">
        <div class="nav-search">
          <div class="input-row">
            ${icons.search('input-icon')}
            <input class="input" type="text" autocomplete="off"
              placeholder="Search anything…" data-nav-input aria-label="Global search"
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
      clearTimeout(this.#debounceTimer);
      this.#debounceTimer = setTimeout(() => {
        this.#refresh();
        this.#searchUsers(this.#input.value.trim());
      }, FILTER_DEBOUNCE_MS);
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
    // With a transparent ::backdrop, clicks outside the panel hit the
    // backdrop (target = the dialog element) — close, don't dim.
    this.#dialog?.addEventListener('click', e => { if (e.target === this.#dialog) this.close(); });
  }

  open() {
    if (!this.#dialog) return;
    this.#usersUnavailable = false;
    this.#refresh();
    this.#position();
    this.#dialog.showModal();
    window.addEventListener('resize', this.#onResize);
    setTimeout(() => this.#input?.focus(), 50);
    this.#loadSources();
  }

  close() {
    if (!this.#dialog) return;
    this.#dialog.close();
    window.removeEventListener('resize', this.#onResize);
    clearTimeout(this.#debounceTimer);
    if (this.#input) this.#input.value = '';
    this.#sections = [];
    this.#flat = [];
    this.#selectedIndex = -1;
  }

  #onResize = () => { if (this.#dialog?.open) this.#position(); };

  /** Anchor the panel under the topbar search field, mockup-style; fall
   *  back to the top-centered CSS position when no anchor is rendered. */
  #position() {
    const anchor = this.#anchor?.isConnected
      ? this.#anchor
      : this.closest('app-header')?.querySelector('[data-search-trigger]');
    const rect = anchor?.getBoundingClientRect();
    const style = this.#dialog.style;
    if (!rect || rect.width === 0) {
      style.margin = style.left = style.top = style.width = '';
      return;
    }
    // Deviation from the 280px mockup panel: our rows carry subtitles, so
    // widen to ~480px (never narrower than the field, never off-screen).
    const width = Math.min(Math.max(rect.width, 480), window.innerWidth - 16);
    const left = Math.min(Math.max(rect.left, 8), window.innerWidth - width - 8);
    style.margin = '0';
    style.left = `${left}px`;
    style.top = `${rect.bottom + 6}px`;
    style.width = `${width}px`;
  }

  /** Fires all entity fetches in parallel; each one re-renders as it lands. */
  #loadSources() {
    const token = ++this.#loadToken;
    const settle = (p, apply) => p
      ?.then((r) => {
        // Ignore stale responses of a previous open, or a closed dialog.
        if (token !== this.#loadToken || !this.#dialog?.open) return;
        apply(r);
        this.#refresh();
      })
      .catch(() => { /* source unavailable — section omitted */ });
    const call = (name, ...args) =>
      typeof window[name] === 'function' ? window[name](...args) : null;

    settle(call('fetchAgents', '', 1, 50), (r) => { this.#data.agents = rowsOf(r); });
    settle(call('fetchWorkflows', 50), (r) => { this.#data.workflows = rowsOf(r); });
    settle(call('fetchAllExecutions', 50), (r) => { this.#data.executions = rowsOf(r); });
    settle(call('fetchSessions', '', 1, 50), (r) => { this.#data.sessions = rowsOf(r); });
    settle(call('fetchMcpConnectors'), (r) => {
      const d = r?.data ?? r ?? {};
      this.#data.connectors = [...(d.created_by_you || []), ...(d.shared_with_you || [])];
    });
    settle(call('fetchMcpToolkits'), (r) => { this.#data.toolkits = r?.data?.toolkits || []; });
    settle(call('fetchBuilds', '', 1, 25), (r) => { this.#data.builds = rowsOf(r); });
    this.#searchUsers(this.#input?.value.trim() || '');
  }

  /** EE-only user directory search; the query runs server-side, so refetch
   *  per (debounced) keystroke. First failure hides the section for this open. */
  async #searchUsers(query) {
    if (this.#usersUnavailable || typeof window.fetchUserSearch !== 'function') return;
    const token = this.#loadToken;
    try {
      const r = await window.fetchUserSearch(query);
      if (token !== this.#loadToken || !this.#dialog?.open) return;
      this.#data.users = Array.isArray(r) ? r : r?.data?.users || r?.data || [];
      this.#refresh();
    } catch {
      this.#usersUnavailable = true;
      this.#data.users = [];
    }
  }

  #refresh() {
    this.#sections = this.#buildSections(this.#input?.value.trim() || '');
    this.#flat = this.#sections.flatMap((s) => s.items);
    this.#selectedIndex = this.#flat.length > 0 ? 0 : -1;
    this.#renderResults();
  }

  /** Grouped, capped, deduped sections; substring match like the rest of the UI. */
  #buildSections(query) {
    const q = query.toLowerCase();
    const hit = (...fields) => !q || fields.some((f) => (f || '').toLowerCase().includes(q));
    const cap = q ? 5 : 3;
    const pfx = this.#userPrefix;
    const seen = new Set();
    const section = (group, icon, rows, match, toItem) => {
      const items = rows.filter(match).map(toItem).filter((item) => {
        const key = `${group}|${item.value}|${item.label}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      });
      return { group, icon, items: items.slice(0, cap), extra: Math.max(0, items.length - cap) };
    };
    const d = this.#data;

    return [
      section('Agents', 'bot', d.agents,
        (a) => hit(a.display_name, a.name, a.description),
        (a) => ({
          label: a.display_name || a.name,
          value: `${pfx}/agent-card.html?id=${encodeURIComponent(a.id)}`,
          subtitle: (a.description || a.name || '').slice(0, 90),
        })),
      section('Workflows', 'workflow', d.workflows,
        (w) => hit(w.name, w.description),
        (w) => ({
          label: w.name,
          value: `${pfx}/workflow.html?id=${encodeURIComponent(w.id)}`,
          subtitle: (w.description || '').slice(0, 90) || w.status,
        })),
      section('Executions', 'play', d.executions.filter((e) => e.maf_id),
        (e) => hit(e.workflow_name, e.status, e.id),
        (e) => ({
          label: `${e.workflow_name || 'Deleted workflow'} · ${e.status}`,
          value: `${pfx}/workflow.html?id=${encodeURIComponent(e.maf_id)}&exec=${encodeURIComponent(e.id)}`,
          subtitle: e.execution_number != null ? `Run #${e.execution_number}` : e.id,
        })),
      section('Chats', 'history', d.sessions,
        (s) => hit(s.title, s.last_message, s.agent_name),
        (s) => ({
          label: (s.title || s.last_message || '').slice(0, 70) || s.session_id,
          value: `${pfx}/chat.html?session_id=${encodeURIComponent(s.session_id)}`
            + `&agent_id=${encodeURIComponent(s.agent_id || '')}`
            + `&agent_name=${encodeURIComponent(s.agent_name || 'Orchestrator')}`,
          subtitle: s.agent_name || 'Orchestrator',
        })),
      section('MCP connectors', 'server', d.connectors,
        (c) => hit(c.display_name, c.name, c.url),
        (c) => ({
          label: c.display_name || c.name,
          value: `${pfx}/mcp.html`,
          subtitle: c.url || c.name,
        })),
      section('Toolkits', 'layers', d.toolkits,
        (t) => hit(t.display_name, t.name, t.description),
        (t) => ({
          label: t.display_name || t.name,
          value: `${pfx}/mcp.html`,
          subtitle: (t.description || '').slice(0, 90)
            || (t.tool_count ? `${t.tool_count} tools` : ''),
        })),
      section('Builds', 'cube', d.builds,
        (b) => hit(b.image_reference, b.version_tag, b.status, b.id),
        (b) => ({
          label: b.image_reference || `Build ${(b.id || '').slice(0, 8)}`,
          value: `${pfx}/build.html?id=${encodeURIComponent(b.id)}`,
          subtitle: [b.status, b.version_tag].filter(Boolean).join(' · '),
        })),
      section('Users', 'users', d.users,
        (u) => hit(u.display_name, u.username, u.email),
        (u) => ({
          label: u.display_name || u.username || u.email,
          value: `${pfx}/users.html`,
          subtitle: u.email || u.role || '',
        })),
      section('Pages', 'document', this.#navLinks,
        (l) => hit(l.title, l.url, l.description),
        (l) => ({
          label: l.title,
          value: pfx + l.url,
          subtitle: l.description || l.url,
        })),
    ].filter((s) => s.items.length > 0);
  }

  #normalize(p) {
    return p.replace(/\/index\.html$/, '/').replace(/\/$/, '') || '/';
  }

  #renderResults() {
    if (!this.#resultsList) return;

    if (this.#flat.length === 0) {
      this.#resultsList.innerHTML = `
        <li class="empty">
          ${icons.faceFrown('empty-icon')}
          <span class="empty-text">No matches found</span>
        </li>`;
      return;
    }

    const currentPath = this.#normalize(window.location.pathname);
    let idx = 0;
    this.#resultsList.innerHTML = this.#sections.map((sec) => {
      const iconHtml = (icons[sec.icon] || icons.document)('result-icon');
      const rows = sec.items.map((item) => {
        const i = idx++;
        const isCurrent = this.#normalize(item.value) === currentPath;
        const indicator = isCurrent
          ? `<span class="result-current-dot" aria-hidden="true"></span>
             <span class="sr-only">(current page)</span>`
          : `${icons.chevronRight('result-arrow')}`;
        return `
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
      const more = sec.extra > 0
        ? `<li class="group-more" role="presentation">+${sec.extra} more — keep typing to narrow</li>`
        : '';
      return `<li class="group-head" role="presentation">${this.#esc(sec.group)}</li>${rows}${more}`;
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
    if (!this.#flat.length) return;
    this.#selectedIndex = (this.#selectedIndex + dir + this.#flat.length) % this.#flat.length;
    this.#updateHighlight();
    this.#resultsList?.querySelector('.is-active')?.scrollIntoView({ block: 'nearest' });
  }

  #confirm(newTab = false) {
    if (this.#selectedIndex === -1 && this.#flat.length > 0) this.#selectedIndex = 0;
    const item = this.#flat[this.#selectedIndex];
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
