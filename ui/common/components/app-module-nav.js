/**
 * In-card module tree navigation (NightOwl): module icon + title header,
 * collapsible groups, and 28px rows with a sand-100 active state.
 *
 * Data comes from `window.fetchModuleNav(module)` (navigation.js), which
 * resolves to `{ title, icon, groups: [{ label, items }] }` where an item is
 * either `{ label, url }` (link, active by path match) or
 * `{ label, section }` (in-page section — clicking dispatches a bubbling
 * `module-nav-select` CustomEvent with `{ section }` for the host page).
 *
 * Desktop (≥1024px): a 200px column pinned to the content card's left edge —
 * the host page component gets matching left padding from
 * `common/styles/page-layout.css`. Mobile: a collapsible disclosure in normal
 * flow above the page content.
 *
 * @element app-module-nav
 * @attr {string} module - Key passed to `window.fetchModuleNav`.
 * @attr {string} active-section - Section key rendered as active (for pages
 *                                 whose sections are tabs, e.g. Settings).
 * @fires module-nav-select - `{ detail: { section } }` on section item click.
 */
import { icons } from "../utils/icons.js";

const styles = new CSSStyleSheet();
styles.replaceSync(`/* Host-page layout contract: the page component that contains a module nav is
   the white content card; the nav pins inside its left padding. The gutter
   itself (the host's padding-left) is page geometry and lives in
   common/styles/page-layout.css — it has to exist at first paint, and this
   sheet only arrives with this module. What is left here is how the nav fills
   that gutter, which is inert until the nav upgrades anyway. */
@media (min-width: 1024px) {
  body:has(> app-header) > :not(app-header):has(> app-module-nav) {
    position: relative;
  }
  body:has(> app-header) > :not(app-header) > app-module-nav {
    position: absolute;
    top: var(--s-24);
    left: var(--s-24);
    bottom: var(--s-24);
    width: 200px;
    overflow-y: auto;
    overflow-x: hidden;
    scrollbar-width: thin;
  }
}

app-module-nav:not(:defined) { display: block; }

@scope (app-module-nav) {
  :scope {
    display: block;
    font-family: var(--font-sans);
  }

  .mod-head {
    display: flex;
    align-items: center;
    gap: var(--s-8);
    height: var(--control-h-sm);
    padding: 0 var(--s-4);
    color: var(--fg-primary);
  }
  .mod-head svg { flex-shrink: 0; color: var(--fg-primary); }
  .mod-title {
    font-size: 14px;
    line-height: 20px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .mod-groups {
    display: flex;
    flex-direction: column;
    gap: var(--s-8);
    margin-top: var(--s-8);
  }

  .group { display: flex; flex-direction: column; gap: var(--s-4); }

  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    height: var(--control-h-sm);
    min-height: var(--control-h-sm);
    padding: 0 var(--s-8);
    border: none;
    border-radius: var(--r-8);
    background: transparent;
    font-family: inherit;
    font-size: 13px;
    line-height: 18px;
    letter-spacing: 0.16px;
    text-align: left;
    text-decoration: none;
    cursor: pointer;
    color: var(--fg-secondary);
    transition: background var(--transition-fast), color var(--transition-fast);
  }
  .row:hover { background: var(--bg-input); }
  .row:focus-visible {
    outline: 2px solid var(--fg-brand);
    outline-offset: -2px;
  }
  .row .row-label {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .group-head {
    color: var(--fg-primary);
    font-weight: 500;
  }
  .group-head .chev {
    display: inline-flex;
    flex-shrink: 0;
    color: var(--fg-secondary);
    transition: rotate var(--transition-fast);
  }
  .group.is-collapsed .group-head .chev { rotate: -90deg; }

  /* Collapsible children: grid-rows 1fr→0fr animates without JS measuring */
  .group-items {
    display: grid;
    grid-template-rows: 1fr;
    transition: grid-template-rows var(--transition-base);
  }
  .group.is-collapsed .group-items { grid-template-rows: 0fr; }
  .group-items > .items-clip {
    display: flex;
    flex-direction: column;
    gap: var(--s-4);
    min-height: 0;
    overflow: hidden;
  }

  .child { padding-left: 26px; }
  .child.is-active {
    background: light-dark(var(--sand-100), var(--neutral-700));
    color: var(--fg-primary);
    font-weight: 500;
  }
  .child.is-active:hover { background: light-dark(var(--sand-100), var(--neutral-700)); }

  /* Skeleton while fetchModuleNav resolves */
  .skel-row {
    height: var(--control-h-sm);
    border-radius: var(--r-8);
    background: var(--bg-input);
    animation: amn-pulse 1.4s ease-in-out infinite;
  }
  .skel-row.is-head { width: 70%; }

  /* Mobile: disclosure above the page content */
  .mobile-toggle { display: none; }
  @media (max-width: 1023.98px) {
    :scope {
      margin-bottom: var(--space-md);
      border: 1px solid var(--border-primary);
      border-radius: var(--r-8);
      padding: var(--s-8);
    }
    .mobile-toggle {
      display: flex;
      width: 100%;
      align-items: center;
      gap: var(--s-8);
      border: none;
      background: transparent;
      font-family: inherit;
      cursor: pointer;
      padding: 0 var(--s-4);
    }
    .mobile-toggle .chev {
      display: inline-flex;
      margin-left: auto;
      color: var(--fg-secondary);
      transition: rotate var(--transition-fast);
    }
    :scope:not(.mobile-open) .mod-head { display: none; }
    :scope:not(.mobile-open) .mod-groups { display: none; }
    :scope.mobile-open .mobile-toggle .chev { rotate: 180deg; }
    :scope.mobile-open .mod-head { display: none; }
  }
  @media (min-width: 1024px) {
    .mobile-toggle { display: none !important; }
  }

  @media (prefers-reduced-motion: reduce) {
    .row, .group-head .chev, .group-items, .mobile-toggle .chev { transition: none; }
    .skel-row { animation: none; opacity: 0.6; }
  }
}

@keyframes amn-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.45; }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

/* Collapsed groups outlive the element. 20 page components render
   `<app-module-nav>` inside their own innerHTML, so every data refresh
   destroys this element and builds a new one — per-instance state meant a
   group the user had closed sprang back open each time, which reads as the
   sidebar resetting itself.
   ponytail: keyed by module in memory, which is all an MPA page lifetime
   needs. The structural fix is to stop page components owning this markup —
   see the note on #load(). */
const COLLAPSED = new Map();

export class AppModuleNav extends HTMLElement {
  #nav = null;
  #mobileOpen = false;

  get #collapsed() {
    const module = this.getAttribute("module") || "";
    let set = COLLAPSED.get(module);
    if (!set) COLLAPSED.set(module, (set = new Set()));
    return set;
  }

  static get observedAttributes() {
    return ["module", "active-section"];
  }

  attributeChangedCallback(name) {
    if (!this.isConnected) return;
    if (name === "module") this.#load();
    else this.#applyActiveSection();
  }

  /** The active section is only ever a class on one row, so move the class
   *  instead of rebuilding the whole tree on every section click. */
  #applyActiveSection() {
    const active = this.getAttribute("active-section");
    for (const row of this.querySelectorAll("[data-section]")) {
      const on = row.dataset.section === active;
      row.classList.toggle("is-active", on);
      if (on) row.setAttribute("aria-current", "true");
      else row.removeAttribute("aria-current");
    }
  }

  connectedCallback() {
    this.addEventListener("click", this.#handleClick);
    this.#load();
  }

  disconnectedCallback() {
    this.removeEventListener("click", this.#handleClick);
  }

  /** Pages may set data directly instead of going through fetchModuleNav. */
  set nav(value) {
    this.#nav = value;
    this.#render();
  }

  async #load() {
    const module = this.getAttribute("module");
    if (!module || typeof window.fetchModuleNav !== "function") {
      this.#nav = null;
      this.#render();
      return;
    }

    // Per-tab cache, same reasoning as app-header's: this is an MPA, so
    // without it every navigation shows a skeleton and then swaps in an
    // identical tree. It is also what makes a content refresh invisible —
    // 20 page components render this element inside their own innerHTML, so
    // an API-driven re-render destroys and recreates it, and the cache lets
    // the replacement paint the same tree synchronously instead of flashing a
    // skeleton. Role-gated trees are dropped on logout by `clearShellCache`.
    const cacheKey = `app-module-nav:${module}`;
    let cached = null;
    try {
      const raw = sessionStorage.getItem(cacheKey);
      if (raw) cached = JSON.parse(raw);
    } catch { /* ignore bad cache */ }

    // `.groups?.length` rather than a plain truthiness check: a previous run
    // could have written a degraded (or literal `null`) tree here.
    if (cached?.groups?.length) {
      this.#nav = cached;
      this.#render();
    } else {
      cached = null;
      this.#renderSkeleton();
    }

    let fresh = null;
    try {
      fresh = await window.fetchModuleNav(module);
    } catch (e) {
      console.warn("fetchModuleNav failed:", e);
    }

    // An empty answer is transient far more often than it is real. EE's
    // fetchModuleNav awaits `/org/context` over the network and degrades to a
    // thinner tree (or nothing) whenever that request wobbles, and #render()
    // deletes this element when handed nothing — which is why the nested
    // sidebar sometimes vanished mid-session on an API call. One flaky request
    // is not a reason to delete a sidebar: keep what is on screen, don't cache
    // the degraded answer over the good one, and let the next load correct it.
    if (!fresh?.groups?.length) {
      if (!cached) {
        this.#nav = fresh;
        this.#render();
      }
      return;
    }

    try {
      sessionStorage.setItem(cacheKey, JSON.stringify(fresh));
    } catch { /* quota exceeded */ }

    // Skip the repaint when the freshly fetched tree matches what's rendered.
    if (cached && JSON.stringify(cached) === JSON.stringify(fresh)) return;
    this.#nav = fresh;
    this.#render();
  }

  #handleClick = (e) => {
    if (e.target.closest("[data-mobile-toggle]")) {
      this.#mobileOpen = !this.#mobileOpen;
      this.classList.toggle("mobile-open", this.#mobileOpen);
      return;
    }
    const head = e.target.closest("[data-group]");
    if (head) {
      const label = head.dataset.group;
      this.#collapsed.has(label) ? this.#collapsed.delete(label) : this.#collapsed.add(label);
      head.closest(".group")?.classList.toggle("is-collapsed", this.#collapsed.has(label));
      head.setAttribute("aria-expanded", String(!this.#collapsed.has(label)));
      return;
    }
    const section = e.target.closest("[data-section]");
    if (section) {
      // A module can mix in-page sections with sibling pages (Settings' panels
      // + /secrets.html). From the sibling page there is no panel to switch, so
      // follow the link and let the owning page pick the section off the hash —
      // swallowing the click there was what pinned the content to Secrets.
      const href = section.getAttribute("href");
      if (href && !this.#isActive(href.split("#")[0])) {
        document.dispatchEvent(new CustomEvent("loading-start", { bubbles: true }));
        return;
      }
      this.setAttribute("active-section", section.dataset.section);
      this.dispatchEvent(new CustomEvent("module-nav-select", {
        bubbles: true,
        detail: { section: section.dataset.section },
      }));
      return;
    }
    if (e.target.closest("a.row[href]")) {
      document.dispatchEvent(new CustomEvent("loading-start", { bubbles: true }));
    }
  };

  #esc(str) {
    if (str == null) return "";
    return String(str).replace(/[&<>"']/g, (m) => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#039;",
    })[m]);
  }

  #normalizePath(p) {
    return p
      .replace(/\/index\.html$/, "/")
      .replace(/\.html$/, "")
      .replace(/\/+$/, "") || "/";
  }

  #isActive(url) {
    const [path, query] = url.split("?");
    if (this.#normalizePath(path) !== this.#normalizePath(window.location.pathname)) return false;
    if (!query) return true;
    const want = new URLSearchParams(query);
    const have = new URLSearchParams(window.location.search);
    return [...want].every(([k, v]) => have.get(k) === v);
  }

  #renderSkeleton() {
    this.innerHTML = `
      <div class="mod-groups" aria-hidden="true" aria-busy="true">
        <div class="skel-row is-head"></div>
        ${Array.from({ length: 4 }, () => `<div class="skel-row"></div>`).join("")}
      </div>`;
  }

  #itemHtml(item) {
    if (item.section != null) {
      const active = this.getAttribute("active-section") === item.section;
      // `url` names the page that owns the sections: a link so the row works
      // from anywhere in the module, and on the owning page it only moves the
      // hash (no reload) while the click handler switches the panel.
      const tag = item.url
        ? `a href="${this.#esc(item.url)}#${this.#esc(item.section)}"`
        : `button type="button"`;
      return `<${tag} class="row child${active ? " is-active" : ""}"
        data-section="${this.#esc(item.section)}" ${active ? 'aria-current="true"' : ""}>
        <span class="row-label">${this.#esc(item.label)}</span></${item.url ? "a" : "button"}>`;
    }
    const active = this.#isActive(item.url);
    return `<a class="row child${active ? " is-active" : ""}" href="${this.#esc(item.url)}"
      ${active ? 'aria-current="page"' : ""}><span class="row-label">${this.#esc(item.label)}</span></a>`;
  }

  #render() {
    const nav = this.#nav;
    if (!nav || !nav.groups?.length) {
      // Remove entirely — a hidden element would still match the host page's
      // `:has(> app-module-nav)` padding rule and leave a dead gutter.
      this.remove();
      return;
    }

    // Default active section: the attribute, else the first section item that
    // belongs to the page we're on — a section owned by another page would
    // otherwise light up next to that page's own active row.
    if (!this.getAttribute("active-section")) {
      const first = nav.groups
        .flatMap((g) => g.items)
        .find((i) => i.section != null && (!i.url || this.#isActive(i.url)));
      if (first) this.setAttribute("active-section", first.section);
    }

    const iconHtml = nav.icon && icons[nav.icon] ? icons[nav.icon]("", 14) : "";
    this.innerHTML = `
      <button class="mobile-toggle" data-mobile-toggle type="button"
        aria-expanded="${this.#mobileOpen}">
        ${iconHtml}
        <span class="mod-title">${this.#esc(nav.title)}</span>
        <span class="chev">${icons.chevronDown("", 14)}</span>
      </button>
      <div class="mod-head">
        ${iconHtml}
        <span class="mod-title">${this.#esc(nav.title)}</span>
      </div>
      <nav class="mod-groups" aria-label="${this.#esc(nav.title)} navigation">
        ${nav.groups.map((g) => `
          <div class="group${this.#collapsed.has(g.label) ? " is-collapsed" : ""}">
            <button type="button" class="row group-head" data-group="${this.#esc(g.label)}"
              aria-expanded="${!this.#collapsed.has(g.label)}">
              <span class="chev">${icons.chevronDown("", 12)}</span>
              <span class="row-label">${this.#esc(g.label)}</span>
            </button>
            <div class="group-items">
              <div class="items-clip">
                ${g.items.map((item) => this.#itemHtml(item)).join("")}
              </div>
            </div>
          </div>
        `).join("")}
      </nav>`;
  }
}

customElements.define("app-module-nav", AppModuleNav);
