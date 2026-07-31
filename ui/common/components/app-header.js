/**
 * Top application header with brand link, navigation, search, and user menu.
 *
 * @element app-header
 * @attr {string} brand-title - Application name shown in the header
 * @attr {string} brand-url - URL the brand title links to (default: `/`)
 * @attr {string} nav-links - JSON array of `{label, url}` objects for nav links
 * @note Includes `<app-nav-search>` and `<app-user-menu>` internally.
 * @note Listens to `loading-start` / `loading-end` on `document` to show a loading bar.
 */
import { authService } from "../services/auth-service.js";
import { icons } from "../utils/icons.js";
import "./app-user-menu.js";
import "./app-nav-search.js";
const styles = new CSSStyleSheet();
styles.replaceSync(`@keyframes ah-skel-pulse {

  0%,
  100% {
    opacity: 1;
  }

  50% {
    opacity: 0.35;
  }
}

::view-transition-old(app-header),
::view-transition-new(app-header) {
  animation: none;
  mix-blend-mode: normal;
}

::view-transition-group(active-nav-pill) {
  animation-duration: 0.25s;
  animation-timing-function: cubic-bezier(0.4, 0, 0.2, 1);
}

@scope (app-header) {
  :scope {
    display: block;
    position: sticky;
    top: 0;
    z-index: 100;
    view-transition-name: app-header;

    @media (min-width: 1024px) {
      position: sticky;
      left: 0;
      top: 0;
      width: var(--app-sidebar-width);
      height: 100dvh;
      flex-shrink: 0;
      transition: width 0.2s ease;
      overflow: visible;
    }
  }

  .nav-link-skel {
    /* 21px bar + 7px margins = the 35px row a rendered .nav-link occupies,
       so the nav doesn't shift when links load. */
    display: inline-block;
    height: 21px;
    margin: 7px var(--space-sm);
    border-radius: var(--radius-sm);
    background: var(--color-border);
    animation: ah-skel-pulse 1.4s ease-in-out infinite;

    @media (min-width: 1024px) {
      margin: 7px var(--space-md);
    }

    @media (prefers-reduced-motion: reduce) {
      animation: none;
      opacity: 0.4;
    }
  }

  .brand-link-skel {
    display: inline-block;
    width: 96px;
    height: 20px;
    border-radius: var(--radius-sm);
    background: var(--color-border);
    flex-shrink: 0;
    animation: ah-skel-pulse 1.4s ease-in-out infinite;

    @media (prefers-reduced-motion: reduce) {
      animation: none;
      opacity: 0.4;
    }
  }

  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-xs);
    padding: 0 var(--space-md);
    height: var(--app-header-height);
    background: var(--color-bg-base);
    border-bottom: none;
    box-shadow: var(--shadow-sm);

    @media (min-width: 1024px) {
      flex-direction: column;
      align-items: stretch;
      height: 100%;
      width: 100%;
      padding: var(--space-md) 0;
      position: static;
      top: unset;
      box-shadow: none;
      background: transparent;
    }
  }

  .brand-row {
    display: none;

    @media (min-width: 1024px) {
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 0 var(--space-sm);
      margin-bottom: var(--space-xs);
      min-height: 36px;
      flex-shrink: 0;
    }
  }

  .brand-link {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: var(--space-xs);
    font-family: var(--font-display);
    font-size: var(--font-size-base);
    font-weight: 500;
    color: var(--color-text-main);
    text-decoration: none;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;

    & .brand-mark { flex-shrink: 0; }

    &:hover {
      color: var(--color-text-main);
    }
  }

  .brand-link-mobile {
    flex-shrink: 0;
    font-size: var(--font-size-sm);
    font-weight: 600;
    color: var(--color-text-main);
    text-decoration: none;

    &:hover {
      color: var(--color-text-main);
    }

    @media (min-width: 1024px) {
      display: none;
    }
  }

  .sidebar-toggle {
    display: none;

    @media (min-width: 1024px) {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 28px;
      height: 28px;
      border-radius: var(--radius-sm);
      color: var(--color-text-muted);
      background: transparent;
      border: none;
      cursor: pointer;
      flex-shrink: 0;

      &:hover {
        color: var(--color-text-main);
        background: color-mix(in srgb, var(--color-primary) 10%, transparent);
      }
    }
  }

  :scope.is-collapsed .brand-link {
    @media (min-width: 1024px) {
      opacity: 0;
      width: 0;
      overflow: hidden;
    }
  }

  .center {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--space-md);
    min-width: 0;

    @media (min-width: 1024px) {
      flex-direction: column;
      align-items: stretch;
      align-self: stretch;
      gap: 0;
      min-height: 0;
      overflow-y: auto;
      scrollbar-width: none;

      &::-webkit-scrollbar {
        display: none;
      }
    }
  }

  .nav {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    position: relative;
    /* Fade the trailing edge so a horizontally scrollable nav reads as scrollable
       instead of hard-clipping labels mid-word. */
    mask-image: linear-gradient(to right, black calc(100% - var(--s-24)), transparent);

    @media (min-width: 1024px) {
      overflow: visible;
      mask-image: none;
    }
  }

  .nav-list {
    display: flex;
    align-items: center;
    gap: var(--space-xs);
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-x: auto;
    scrollbar-width: none;

    & li {
      margin: 0;
      padding: 0;
    }

    &::-webkit-scrollbar {
      display: none;
    }

    @media (min-width: 1024px) {
      flex-direction: column;
      overflow-x: visible;
      gap: var(--s-8);
      align-items: stretch;
      padding: 0 var(--space-sm);
    }
  }

  .nav-icon {
    opacity: 0.6;
    flex-shrink: 0;
  }
  .nav-link:hover .nav-icon, .nav-link.is-active .nav-icon { opacity: 1; }

  .nav-link {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: var(--space-xs);
    padding: var(--space-xs) var(--space-sm);
    border-radius: var(--radius-sm);
    font-size: var(--font-size-sm);
    font-weight: 400;
    color: var(--color-text-main);
    text-decoration: none;
    white-space: nowrap;
    border: 1px solid transparent;
    background-color: transparent;
    transition: color 0.15s;

    &:hover {
      color: var(--color-primary);
      background-color: color-mix(in srgb, var(--color-primary) 8%, transparent);
    }

    &:focus {
      box-shadow: 0 0 0 2px var(--color-primary-ring);
    }

    &.is-active {
      color: var(--color-primary);
      background-color: transparent;
      border-color: transparent;

      &:hover {
        background-color: transparent;
      }
    }

    @media (min-width: 1024px) {
      display: flex;
      align-items: center;
      gap: var(--space-sm);
      text-align: left;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
      padding: 10px var(--space-sm);
      border-radius: var(--r-10);
      background: var(--bg-canvas-card);
      border: 1px solid var(--border-canvas);
      color: var(--color-text-main);
      font-weight: 500;

      &:hover {
        color: var(--color-text-main);
        background-color: light-dark(var(--cream-100), var(--neutral-700));
      }

      &.is-active {
        background-color: var(--bg-secondary-brand);
        border-color: light-dark(var(--yellow-200), var(--yellow-800));
        &:hover {
          background-color: var(--bg-secondary-brand);
        }
      }
    }
  }

  :scope.is-collapsed .nav-link-text {
    @media (min-width: 1024px) {
      opacity: 0;
      width: 0;
      overflow: hidden;
    }
  }

  .nav-link-bg {
    position: absolute;
    inset: 0;
    border-radius: inherit;
    background-color: color-mix(in srgb, var(--color-primary) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--color-primary) 40%, transparent);
    z-index: 1;
    view-transition-name: active-nav-pill;

    @media (min-width: 1024px) {
      background-color: transparent;
      border: none;
    }
  }

  .nav-link-text {
    position: relative;
    z-index: 2;
  }

  .menu-btn {
    flex-shrink: 0;
    align-self: center;
    width: 44px;
    height: 44px;
    border-radius: var(--radius-sm);
    color: var(--color-text-muted);

    &:hover {
      color: var(--color-text-main);
      background: var(--color-bg-base);
    }

    &:focus {
      box-shadow: 0 0 0 2px var(--color-primary-ring);
    }

    @media (min-width: 1024px) {
      display: none;
    }
  }

  .menu-icon {
    width: 20px;
    height: 20px;
    flex-shrink: 0;
  }

  .user {
    flex-shrink: 0;
    align-self: center;
    position: relative;

    @media (min-width: 1024px) {
      margin-top: auto;
      padding: var(--space-xs) var(--space-sm);
      border-top: 1px solid var(--color-border);
      display: flex;
      flex-direction: column;
      align-items: stretch;
      align-self: stretch;
      --user-btn-w: 100%;
      --user-btn-h: 44px;
      --user-btn-justify: flex-start;
      --user-btn-padding: 0 var(--space-sm);
      --user-dropdown-top: auto;
      --user-dropdown-bottom: calc(100% + var(--space-xs));
      --user-dropdown-right: auto;
      --user-dropdown-left: 0;
    }
  }

  :scope.is-collapsed .user {
    @media (min-width: 1024px) {
      --user-name-display: none;
    }
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border-width: 0;

    &.is-focusable:focus {
      position: fixed;
      top: var(--space-sm);
      left: var(--space-sm);
      z-index: 10000;
      width: auto;
      height: auto;
      padding: var(--space-sm) var(--space-md);
      margin: 0;
      overflow: visible;
      clip: auto;
      white-space: normal;
      background: var(--color-primary);
      color: var(--color-on-primary);
      border-radius: var(--radius-md);
      font-size: var(--font-size-sm);
      font-weight: 600;
      text-decoration: none;
    }
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export class AppHeader extends HTMLElement {
  #collapsed = localStorage.getItem("app-sidebar-collapsed") === "true";

  #esc(str) {
    if (!str) return "";
    return str.replace(/[&<>"']/g, m => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;'
    })[m]);
  }

  #handleKeyDown = (e) => {
    const isShortcut =
      ((e.metaKey || e.ctrlKey) && e.key === "k") || e.key === "\\";
    if (isShortcut) {
      e.preventDefault();
      const navSearch = this.querySelector("app-nav-search");
      if (navSearch && !navSearch.querySelector("[data-nav-dialog]")?.open) {
        navSearch.open();
      }
    }
  };

  #handleClick = (e) => {
    if (e.target.closest("[data-sidebar-toggle]")) {
      this.#collapsed = !this.#collapsed;
      localStorage.setItem("app-sidebar-collapsed", this.#collapsed);
      this.#applyCollapsed();
      return;
    }
    if (e.target.closest("[data-search-trigger]")) {
      this.querySelector("app-nav-search")?.open();
    } else {
      const link = e.target.closest(".nav-link");
      if (link && !(e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0)) {
        document.dispatchEvent(new CustomEvent("loading-start", { bubbles: true }));
      }
    }
  };

  #applyCollapsed() {
    this.classList.toggle("is-collapsed", this.#collapsed);
    document.documentElement.style.setProperty(
      "--app-sidebar-width",
      this.#collapsed ? "var(--app-sidebar-width-collapsed)" : "var(--app-sidebar-width-expanded)"
    );
  }

  static get observedAttributes() {
    return ["nav-links", "brand-title", "brand-url"];
  }

  attributeChangedCallback() {
    if (this.isConnected) this.render();
  }

  async connectedCallback() {
    this.#applyCollapsed();
    document.removeEventListener("keydown", this.#handleKeyDown);
    this.removeEventListener("click", this.#handleClick);
    this.addEventListener("click", this.#handleClick);
    if (this.getAttribute("nav-links")) {
      // nav-links attribute present — render synchronously so view-transition-name
      // styles are in the DOM before the browser captures the new-page snapshot.
      this.render();
      document.addEventListener("keydown", this.#handleKeyDown);
      return;
    }
    // No nav-links attribute — async fetch path. Render from sessionStorage cache
    // first so view-transition-names are available before pagereveal, then
    // re-render once fresh data arrives.
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
    // Basename comparison: /u/user/history === /history
    const currentBase = current.split("/").pop();
    const targetBase = target.split("/").pop();
    return !!currentBase && currentBase === targetBase;
  }

  #renderSkeleton() {
    const widths = [56, 72, 48, 64];
    this.innerHTML = `
      <header class="bar" role="banner">
        <div class="center">
          ${this.getAttribute("brand-title") ? `<span class="brand-link-skel" aria-hidden="true"></span>` : ""}
          <nav class="nav" aria-label="Main navigation" aria-busy="true">
            <ul class="nav-list" role="list">
              ${widths.map((w) => `<li><span class="nav-link-skel" style="width:${w}px" aria-hidden="true"></span></li>`).join("")}
            </ul>
          </nav>
        </div>
      </header>
    `;
  }

  render() {
    const navLinksJson = this.getAttribute("nav-links");
    let navLinks = [];
    if (navLinksJson) {
      try {
        navLinks = JSON.parse(navLinksJson);
      } catch (e) {
        console.error("Invalid nav-links JSON:", e);
      }
    } else {
      navLinks = this.navItems || [];
    }

    const currentUser = authService.getCurrentUser();
    const isAuthenticated = authService.isAuthenticated();
    const brandTitle = this.getAttribute("brand-title");
    const brandUrl = this.getAttribute("brand-url") || "/";
    const userPrefix = "";

    const navPills = navLinks
      .filter((link) => {
        if (link.hidden) return false;
        const urlLower = (link.url || "").toLowerCase();
        return !urlLower.includes("/login") && !urlLower.includes("/admin");
      })
      .map((link) => {
        const href = userPrefix + link.url;
        const active = this.#isActive(href);
        const titleEsc = this.#esc(link.title);
        const bgSpan = active ? '<span class="nav-link-bg"></span>' : '';
        const iconHtml = link.icon && icons[link.icon] ? icons[link.icon]('nav-icon', 16) : '';
        return `<li><a href="${this.#esc(href)}"
        class="nav-link${active ? " is-active" : ""}"
        data-text="${titleEsc}"
        title="${titleEsc}"
        ${active ? 'aria-current="page"' : ""}>${bgSpan}${iconHtml}<span class="nav-link-text">${titleEsc}</span></a></li>`;
      })
      .join("");

    this.innerHTML = `
      <a href="#main-content" class="sr-only is-focusable">Skip to main content</a>
      <header class="bar" role="banner">
        <div class="brand-row">
          ${brandTitle ? `<a href="${this.#esc(brandUrl)}" class="brand-link"><img class="brand-mark" src="/common/mark-nasiko.svg" alt="" width="20" height="20" />${this.#esc(brandTitle)}</a>` : ""}
          <button class="sidebar-toggle" data-sidebar-toggle
            aria-label="Toggle sidebar" type="button">
            ${icons.panelLeft("", 18)}
          </button>
        </div>
        <div class="center">
          ${brandTitle ? `<a href="${this.#esc(brandUrl)}" class="brand-link-mobile">${this.#esc(brandTitle)}</a>` : ""}
          ${
            navLinks.length
              ? `
            <nav class="nav" aria-label="Main navigation">
              <ul class="nav-list" role="list">${navPills}</ul>
            </nav>`
              : ""
          }
        </div>
        ${
          navLinks.length
            ? `
          <button class="menu-btn" data-search-trigger
            aria-label="Search pages" type="button">
            ${icons.search("menu-icon", 20)}
          </button>`
            : ""
        }
        ${isAuthenticated ? `<app-user-menu class="user" current-user="${this.#esc(currentUser)}"></app-user-menu>` : ""}
        ${navLinks.length ? `<app-nav-search></app-nav-search>` : ""}
      </header>
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
      navSearch.userPrefix = userPrefix;
      navSearch.addEventListener("navigate", (e) => {
        e.detail.newTab
          ? window.open(e.detail.url, "_blank")
          : (window.location.href = e.detail.url);
      });
    }

    // Scroll active nav link into view on mount
    this.querySelector(".nav-link.is-active")?.scrollIntoView({
      block: "nearest",
      inline: "center",
    });
  }

  #removeUser(username) {
    if (confirm(`Remove account for ${username}?`)) {
      authService.removeUserSession(username);
      // Only the user list changed — update the existing element directly
      // instead of nuking and re-mounting the whole header (Rule 17).
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
