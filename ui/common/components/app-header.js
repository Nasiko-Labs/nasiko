/**
 * NightOwl application shell: 52px ink topbar + collapsible left icon rail.
 *
 * Renders both chrome bars from one element so existing pages keep their
 * single `<app-header>` tag. The rail lists the pages from
 * `window.fetchNavigation()` (or the `nav-links` attribute); Settings and the
 * identity menu pin to the rail's bottom cluster.
 *
 * @element app-header
 * @attr {string} brand-title - Application name (used for tooltips/aria)
 * @attr {string} brand-url - URL the brand mark links to (default: `/`)
 * @attr {string} nav-links - JSON array of `{title, url, icon}` objects
 * @note Includes `<app-nav-search>` and `<app-user-menu>` internally.
 * @note Dispatches `loading-start` on nav clicks (see app-loading-bar).
 */
import { authService } from "../services/auth-service.js";
import { icons } from "../utils/icons.js";
import "./app-user-menu.js";
import "./app-nav-search.js";

const styles = new CSSStyleSheet();
styles.replaceSync(`@keyframes ah-skel-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}

::view-transition-old(app-header),
::view-transition-new(app-header) {
  animation: none;
  mix-blend-mode: normal;
}

@scope (app-header) {
  :scope {
    display: block;
    position: sticky;
    top: 0;
    z-index: 100;
    view-transition-name: app-header;

    @media (min-width: 1024px) {
      position: fixed;
      inset: 0 0 auto 0;
      height: var(--shell-topbar-height);
    }
  }

  /* ── Topbar ─────────────────────────────────────────────────────────── */
  .topbar {
    display: flex;
    align-items: center;
    gap: var(--s-12);
    height: var(--shell-topbar-height);
    padding: 0 var(--s-12);
    background: var(--shell-bg);
    color: var(--shell-fg);
  }

  /* Dark-chrome control recipe — every topbar/rail control shares it */
  .chrome-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--s-8);
    height: var(--control-h-sm);
    min-width: var(--control-h-sm);
    padding: 0;
    border: none;
    border-radius: var(--r-6);
    background: var(--shell-control);
    color: var(--shell-fg);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: background var(--transition-fast);
  }
  .chrome-btn:hover { background: var(--shell-control-hover); }
  .chrome-btn:active { background: var(--shell-control-active); }
  .chrome-btn.is-labeled { padding: 0 var(--s-12); }
  .chrome-btn:focus-visible {
    outline: 2px solid var(--shell-selected);
    outline-offset: 1px;
  }
  a.chrome-btn { text-decoration: none; color: var(--shell-fg); }

  .identity-chip {
    width: 36px;
    height: 36px;
    border-radius: var(--r-8);
    background: var(--blue-600);
    color: var(--white);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 14px;
    font-weight: 600;
    flex-shrink: 0;
    user-select: none;
  }

  .nav-cluster {
    display: flex;
    align-items: center;
    gap: var(--s-12);
  }

  .search-field {
    display: flex;
    align-items: center;
    gap: var(--s-8);
    width: 280px;
    height: var(--control-h-sm);
    padding: 0 var(--s-12);
    border: none;
    border-radius: var(--r-8);
    background: var(--shell-control);
    color: var(--shell-fg);
    font-size: 13px;
    cursor: pointer;
    text-align: left;
  }
  .search-field:hover { background: var(--shell-control-hover); }
  .search-field .placeholder {
    flex: 1;
    color: var(--shell-fg-muted);
    white-space: nowrap;
    overflow: hidden;
  }
  .search-field .kbd-hint { color: var(--shell-fg-muted); font-size: 13px; }
  .search-field svg { color: var(--shell-fg-muted); }

  .topbar-spacer { flex: 1; }

  .topbar-right {
    display: flex;
    align-items: center;
    gap: var(--s-12);
  }

  /* Small screens: drop the history cluster, let search flex, keep menu */
  @media (max-width: 1023.98px) {
    .nav-cluster { display: none; }
    .search-field { width: auto; flex: 1; min-width: 0; }
    .search-field .kbd-hint { display: none; }
    .topbar-spacer { display: none; }
    .topbar-right a.chrome-btn.is-labeled { display: none; }
  }

  /* ── Rail ───────────────────────────────────────────────────────────── */
  .rail {
    display: none;

    @media (min-width: 1024px) {
      display: flex;
      flex-direction: column;
      align-items: stretch;
      gap: var(--s-12);
      position: fixed;
      top: var(--shell-topbar-height);
      bottom: 0;
      left: 0;
      width: var(--app-sidebar-width);
      padding: var(--s-12);
      background: var(--shell-bg);
      color: var(--shell-fg);
      overflow-y: auto;
      overflow-x: hidden;
      scrollbar-width: none;
      transition: width var(--transition-base);
    }

    @media (prefers-reduced-motion: reduce) {
      .rail { transition: none; }
    }
  }
  .rail::-webkit-scrollbar { display: none; }

  .rail-item {
    display: flex;
    align-items: center;
    gap: var(--s-12);
    height: var(--control-h-md);
    min-height: var(--control-h-md);
    width: var(--control-h-md);
    padding: 0;
    justify-content: center;
    border: none;
    border-radius: var(--r-8);
    background: var(--shell-control);
    color: var(--shell-fg);
    font-size: 13px;
    font-weight: 500;
    text-decoration: none;
    cursor: pointer;
    transition: background var(--transition-fast);
    white-space: nowrap;
  }
  .rail-item:hover { background: var(--shell-control-hover); color: var(--shell-fg); }
  .rail-item:active { background: var(--shell-control-active); }
  .rail-item.is-active {
    background: transparent;
    color: var(--shell-selected);
  }
  .rail-item:focus-visible {
    outline: 2px solid var(--shell-selected);
    outline-offset: 1px;
  }
  .rail-item .rail-label { display: none; }

  :scope.is-expanded .rail-item {
    width: 100%;
    justify-content: flex-start;
    padding: 0 var(--s-8);
    background: transparent;
  }
  :scope.is-expanded .rail-item:hover { background: var(--shell-control-hover); }
  :scope.is-expanded .rail-item.is-active {
    background: var(--yellow-100);
    color: var(--sand-900);
  }
  :scope.is-expanded .rail-item .rail-label {
    display: inline;
    animation: panel-in var(--transition-base) 60ms backwards;
  }
  :scope.is-expanded .rail-identity .identity-name {
    animation: panel-in var(--transition-base) 60ms backwards;
  }

  @media (prefers-reduced-motion: reduce) {
    :scope.is-expanded .rail-item .rail-label,
    :scope.is-expanded .rail-identity .identity-name { animation: none; }
  }

  .rail-bottom {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: var(--s-12);
  }

  .rail-identity {
    display: flex;
    align-items: center;
    gap: var(--s-12);
  }
  .rail-identity app-user-menu {
    flex-shrink: 0;
    /* Match the 32px rail buttons: square avatar aligned to the icon column
       instead of the default 36px circle; the inline name/chevron stay
       hidden — the rail shows its own label when expanded. */
    --user-btn-w: var(--control-h-md);
    --user-btn-h: var(--control-h-md);
    --user-avatar-size: 30px;
    --user-avatar-radius: var(--r-6);
    --user-name-display: none;
    /* The rail scroll-clips absolute descendants — open the menu as a
       viewport-fixed flyout beside the rail's bottom cluster instead.
       ('auto', not 'unset': a CSS-wide keyword in a custom property makes it
       guaranteed-invalid, so var() would take its fallback instead.) */
    --user-dropdown-position: fixed;
    --user-dropdown-top: auto;
    --user-dropdown-bottom: var(--s-12);
    --user-dropdown-left: calc(var(--app-sidebar-width) + var(--s-4));
    --user-dropdown-right: auto;
  }
  .rail-identity .identity-name {
    display: none;
    font-size: 13px;
    font-weight: 600;
    color: var(--shell-fg);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  :scope.is-expanded .rail-identity .identity-name { display: block; }

  /* ── Mobile ─────────────────────────────────────────────────────────── */
  .mobile-nav {
    display: none;
  }
  :scope.mobile-open .mobile-nav {
    display: flex;
    flex-direction: column;
    gap: var(--s-4);
    padding: var(--s-8) var(--s-12) var(--s-12);
    background: var(--shell-bg);
  }
  .mobile-nav .rail-item {
    width: 100%;
    justify-content: flex-start;
    padding: 0 var(--s-8);
    background: transparent;
  }
  .mobile-nav .rail-item .rail-label { display: inline; }
  .mobile-nav .rail-item.is-active { background: var(--yellow-100); color: var(--sand-900); }

  .mobile-menu-btn { display: inline-flex; }

  @media (min-width: 1024px) {
    .mobile-menu-btn { display: none; }
    .mobile-nav, :scope.mobile-open .mobile-nav { display: none; }
  }

  /* ── Skeleton ───────────────────────────────────────────────────────── */
  .rail-skel {
    height: var(--control-h-md);
    width: var(--control-h-md);
    border-radius: var(--r-8);
    background: var(--shell-control);
    animation: ah-skel-pulse 1.4s ease-in-out infinite;

    @media (prefers-reduced-motion: reduce) {
      animation: none;
      opacity: 0.4;
    }
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export class AppHeader extends HTMLElement {
  #expanded = localStorage.getItem("app-rail-expanded") === "true";
  #mobileOpen = false;

  #esc(str) {
    if (!str) return "";
    return str.replace(/[&<>"']/g, m => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;'
    })[m]);
  }

  #handleKeyDown = (e) => {
    const isShortcut =
      ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "f")) || e.key === "\\";
    if (isShortcut) {
      const navSearch = this.querySelector("app-nav-search");
      if (!navSearch) return;
      e.preventDefault();
      if (!navSearch.querySelector("[data-nav-dialog]")?.open) navSearch.open();
    }
  };

  #handleClick = (e) => {
    if (e.target.closest("[data-rail-toggle]")) {
      this.#expanded = !this.#expanded;
      localStorage.setItem("app-rail-expanded", this.#expanded);
      this.#applyExpanded();
      return;
    }
    if (e.target.closest("[data-mobile-menu]")) {
      this.#mobileOpen = !this.#mobileOpen;
      this.classList.toggle("mobile-open", this.#mobileOpen);
      return;
    }
    if (e.target.closest("[data-search-trigger]")) {
      this.querySelector("app-nav-search")?.open();
      return;
    }
    if (e.target.closest("[data-nav-back]")) { window.history.back(); return; }
    if (e.target.closest("[data-nav-fwd]")) { window.history.forward(); return; }
    const link = e.target.closest(".rail-item[href]");
    if (link && !(e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0)) {
      document.dispatchEvent(new CustomEvent("loading-start", { bubbles: true }));
    }
  };

  #applyExpanded() {
    this.classList.toggle("is-expanded", this.#expanded);
    document.documentElement.style.setProperty(
      "--app-sidebar-width",
      this.#expanded ? "var(--app-sidebar-width-expanded)" : "var(--app-sidebar-width-collapsed)"
    );
  }

  static get observedAttributes() {
    return ["nav-links", "brand-title", "brand-url"];
  }

  attributeChangedCallback() {
    if (this.isConnected) this.render();
  }

  async connectedCallback() {
    this.#applyExpanded();
    document.removeEventListener("keydown", this.#handleKeyDown);
    this.removeEventListener("click", this.#handleClick);
    this.addEventListener("click", this.#handleClick);
    if (this.getAttribute("nav-links")) {
      this.render();
      document.addEventListener("keydown", this.#handleKeyDown);
      return;
    }
    const cached = sessionStorage.getItem("app-header-nav");
    if (cached) {
      try { this.navItems = JSON.parse(cached); } catch { /* ignore bad cache */ }
    }
    if (this.navItems?.length) {
      this.render();
    } else {
      this.#renderSkeleton();
    }
    await Promise.all([this.loadNavigation(), authService.fetchCurrentUser()]);
    this.render();
    document.addEventListener("keydown", this.#handleKeyDown);
  }

  disconnectedCallback() {
    document.removeEventListener("keydown", this.#handleKeyDown);
    this.removeEventListener("click", this.#handleClick);
  }

  async loadNavigation() {
    if (this.getAttribute("nav-links")) return;
    if (typeof window.fetchNavigation === "function") {
      try {
        this.navItems = await window.fetchNavigation();
        try { sessionStorage.setItem("app-header-nav", JSON.stringify(this.navItems)); } catch { /* quota exceeded */ }
      } catch (e) {
        console.warn("fetchNavigation failed:", e);
        if (!this.navItems) this.navItems = [];
      }
    } else {
      if (!this.navItems) this.navItems = [];
    }
  }

  #normalizePath(p) {
    return p
      .replace(/\/index\.html$/, "/")
      .replace(/\.html$/, "")
      .replace(/\/+$/, "") || "/";
  }

  /** Returns true if href matches the current page, tolerating user-prefix differences. */
  #isActive(href) {
    const current = this.#normalizePath(window.location.pathname);
    const target = this.#normalizePath(href);
    if (target === current) return true;
    const currentBase = current.split("/").pop();
    const targetBase = target.split("/").pop();
    return !!currentBase && currentBase === targetBase;
  }

  #initials() {
    const user = authService.getCurrentUser() || "";
    const parts = user.split(/[\s._-]+/).filter(Boolean);
    if (!parts.length) return "N";
    return parts.slice(0, 2).map(p => p[0].toUpperCase()).join("");
  }

  #navLinks() {
    const navLinksJson = this.getAttribute("nav-links");
    if (navLinksJson) {
      try { return JSON.parse(navLinksJson); } catch (e) {
        console.error("Invalid nav-links JSON:", e);
        return [];
      }
    }
    return this.navItems || [];
  }

  #railItem(link) {
    const href = link.url;
    const active = this.#isActive(href);
    const titleEsc = this.#esc(link.title);
    // Chrome icons (rail + topbar) render at 1px stroke per the NightOwl weight rule.
    // Rail glyphs: 1.25 stroke — the mockup's 1px chrome weight reads wispy at
    // 18px on the ink rail; topbar utility icons stay at 1.
    const iconHtml = link.icon && icons[link.icon] ? icons[link.icon]('', 18, 1.25) : icons.cube('', 18, 1.25);
    return `<a href="${this.#esc(href)}" class="rail-item${active ? " is-active" : ""}"
      title="${titleEsc}" ${active ? 'aria-current="page"' : ""}>${iconHtml}<span class="rail-label">${titleEsc}</span></a>`;
  }

  #renderSkeleton() {
    this.innerHTML = `
      <header class="topbar" role="banner">
        <span class="identity-chip" aria-hidden="true"></span>
      </header>
      <nav class="rail" aria-label="Main navigation" aria-busy="true">
        ${Array.from({ length: 6 }, () => `<span class="rail-skel" aria-hidden="true"></span>`).join("")}
      </nav>
    `;
  }

  render() {
    const navLinks = this.#navLinks().filter((link) => {
      if (link.hidden) return false;
      const urlLower = (link.url || "").toLowerCase();
      return !urlLower.includes("/login") && !urlLower.includes("/admin");
    });
    const isAuthenticated = authService.isAuthenticated();
    const currentUser = authService.getCurrentUser();

    const settingsLinks = navLinks.filter(l => /settings/i.test(l.title));
    const mainLinks = navLinks.filter(l => !settingsLinks.includes(l));
    const addAgent = mainLinks.find(l => /add agent/i.test(l.title));
    // Rail shows module-level entries (rail: true); when no item carries the
    // flag (attribute-driven navs, EE overrides) every link is a rail item.
    const hasRailFlags = mainLinks.some(l => l.rail);
    const railLinks = mainLinks.filter(l => l !== addAgent && (!hasRailFlags || l.rail));

    this.innerHTML = `
      <a href="#main-content" class="sr-only is-focusable">Skip to main content</a>
      <header class="topbar" role="banner">
        <span class="identity-chip" title="${this.#esc(currentUser || "Nasiko")}">${this.#esc(this.#initials())}</span>
        <button class="chrome-btn" data-rail-toggle aria-label="Toggle sidebar" type="button">
          ${icons.panelLeft("", 16, 1)}
        </button>
        <div class="nav-cluster">
          <button class="chrome-btn" data-nav-back aria-label="Back" type="button">${icons.chevronLeft("", 16, 1)}</button>
          <button class="chrome-btn" data-nav-fwd aria-label="Forward" type="button">${icons.chevronRight("", 16, 1)}</button>
        </div>
        ${navLinks.length ? `
        <button class="search-field" data-search-trigger type="button" aria-label="Search pages">
          ${icons.search("", 16, 1)}
          <span class="placeholder">Search anything...</span>
          <span class="kbd-hint">\u2318F</span>
        </button>` : ""}
        <span class="topbar-spacer"></span>
        <div class="topbar-right">
          ${addAgent ? `<a href="${this.#esc(addAgent.url)}" class="chrome-btn is-labeled">${icons.plus("", 16, 1)} Add agent</a>` : ""}
          <button class="chrome-btn mobile-menu-btn" data-mobile-menu aria-label="Menu" type="button">${icons.menu("", 16, 1)}</button>
        </div>
      </header>
      <nav class="rail" aria-label="Main navigation">
        ${railLinks.map(l => this.#railItem(l)).join("")}
        <div class="rail-bottom">
          ${settingsLinks.map(l => this.#railItem(l)).join("")}
          ${isAuthenticated ? `
          <div class="rail-identity">
            <app-user-menu current-user="${this.#esc(currentUser)}"></app-user-menu>
            <span class="identity-name">${this.#esc(currentUser)}</span>
          </div>` : ""}
        </div>
      </nav>
      <div class="mobile-nav">
        ${mainLinks.concat(settingsLinks).map(l => this.#railItem(l)).join("")}
      </div>
      ${navLinks.length ? `<app-nav-search></app-nav-search>` : ""}
    `;

    const userMenu = this.querySelector("app-user-menu");
    if (userMenu) {
      userMenu.users = authService.getUsers();
      userMenu.addEventListener("user-remove", (e) =>
        this.#removeUser(e.detail.username),
      );
      userMenu.addEventListener("user-add-account", () => this.#addAccount());
      userMenu.addEventListener("user-logout", () => this.#logout());
    }

    const navSearch = this.querySelector("app-nav-search");
    if (navSearch) {
      navSearch.navLinks = navLinks;
      navSearch.userPrefix = "";
      // The palette drops down anchored under the topbar search field.
      navSearch.anchorEl = this.querySelector("[data-search-trigger]");
      navSearch.addEventListener("navigate", (e) => {
        e.detail.newTab
          ? window.open(e.detail.url, "_blank")
          : (window.location.href = e.detail.url);
      });
    }
  }

  #removeUser(username) {
    if (confirm(`Remove account for ${username}?`)) {
      authService.removeUserSession(username);
      const userMenu = this.querySelector("app-user-menu");
      if (userMenu) userMenu.users = authService.getUsers();
    }
  }

  #addAccount() {
    window.location.href =
      "/login/?add_account=true&redirect=" +
      encodeURIComponent(window.location.pathname);
  }

  #logout() {
    const currentUser = authService.getCurrentUser();
    if (currentUser && confirm(`Sign out from ${currentUser}?`)) {
      authService.removeUserSession(currentUser);
      const remaining = authService.getUsers();
      window.location.href =
        remaining.length > 0
          ? `/u/${remaining[0].username}/`
          : "/login/index.html";
    }
  }
}

customElements.define("app-header", AppHeader);
