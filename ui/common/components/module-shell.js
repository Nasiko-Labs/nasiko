/**
 * @element module-shell
 *
 * Host for one module's views. A module (Orchestrator, Agents, Observability,
 * Settings) is one document holding every view in its nested sidebar, and this
 * element is what makes switching between them free: it owns the
 * `app-module-nav`, so a switch shows and hides content underneath a sidebar
 * that is never re-rendered, never re-fetched, and never re-animated. Only
 * moving between *modules* is a real navigation.
 *
 * Markup contract — the default view is real markup so it paints before any
 * module loads; every other view is a `<template>` so its component does not
 * upgrade (and does not hit the API) until first shown:
 *
 *   <module-shell module="agents">
 *     <app-module-nav module="agents"></app-module-nav>
 *     <agents-page data-view="hub" data-title="Agent hub">…static shell…</agents-page>
 *     <template data-view="builds" data-title="Builds"
 *               data-module="/common/components/builds-page.js">
 *       <builds-page></builds-page>
 *     </template>
 *   </module-shell>
 *
 * `data-module` is imported the first time its view is shown, so a module page
 * costs exactly one view's JS on load rather than all of them. The element
 * upgrades on its own when that import defines it. (Their stylesheets are still
 * `<link>`ed by the page — a sheet that arrives with the module is too late to
 * style anything it paints.)
 *
 * The `data-view` keys must match the `section` keys this module's entry in
 * `MODULE_NAVS` (navigation.js) declares — that pairing is the whole nav
 * contract. This element resolves the active view itself and pushes it onto the
 * nav, so the nav never has to guess and the two cannot disagree.
 *
 * Layout: as the direct body child this element is the white content card (see
 * the card rule in global.css), and on desktop it carries the nav's left gutter
 * — page geometry, so both live in common/styles/page-layout.css.
 */
import { VIEW_PARAM, initialView, syncView } from "../utils/module-view.js";
import "./app-module-nav.js";

export class ModuleShell extends HTMLElement {
  #initialized = false;
  #active = null;
  /** view key → the element rendering it, once mounted. */
  #mounted = new Map();
  /** view key → the `<template>` it has yet to be mounted from. */
  #pending = new Map();
  #baseTitle = "";

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#baseTitle = document.title;

    for (const child of this.children) {
      const key = child.dataset?.view;
      if (!key) continue;
      if (child instanceof HTMLTemplateElement) this.#pending.set(key, child);
      else this.#mounted.set(key, child);
    }

    const views = [...this.#mounted.keys(), ...this.#pending.keys()];
    if (!views.length) return;

    this.addEventListener("module-nav-select", this.#onSelect);
    this.addEventListener("click", this.#onClick);

    // The inline view is the one already painted, so it is the natural default:
    // showing anything else means a swap the user sees, and only an explicit
    // `?view=` justifies that.
    const fallback = this.getAttribute("default-view")
      || [...this.#mounted.keys()][0]
      || views[0];
    const initial = initialView(views, fallback);

    // Hide the inline views up front. They are painted markup, not a swap this
    // element made, so `#show` has no previous view to hide on first run — and
    // without this the default view stayed on screen underneath the one
    // `?view=` asked for. Done in the same task as the reveal below, so nothing
    // repaints in between.
    for (const [key, el] of this.#mounted) {
      if (key !== initial) el.setAttribute("hidden", "");
    }

    this.#show(initial, { sync: false });
  }

  disconnectedCallback() {
    this.removeEventListener("module-nav-select", this.#onSelect);
    this.removeEventListener("click", this.#onClick);
  }

  /**
   * A link to a sibling view of this same document switches in place.
   *
   * Views cross-link each other ("Browse workflows" from the executions empty
   * state, "Back to builds" from a detail page), and those hrefs have to stay
   * real URLs — they must work from another page and survive being copied. But
   * followed from inside the module they would reload the document the user is
   * already on to reach content already loaded. Modifier and middle clicks are
   * left alone: those mean "open elsewhere", which is still a navigation.
   */
  #onClick = (e) => {
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    if (e.button !== 0) return;
    const link = e.target.closest("a[href]");
    if (!link || link.target === "_blank") return;

    const url = new URL(link.href, location.href);
    if (url.origin !== location.origin || url.pathname !== location.pathname) return;

    const view = url.searchParams.get(VIEW_PARAM);
    if (!view || view === this.#active) return;
    if (!this.#mounted.has(view) && !this.#pending.has(view)) return;

    e.preventDefault();
    this.#show(view);
  };

  #onSelect = (e) => {
    const view = e.detail?.section;
    // The nav already wrote the URL and moved its own highlight; this element
    // only has to swap the content.
    if (view) this.#show(view, { sync: false });
  };

  /** The view currently on screen. */
  get activeView() {
    return this.#active;
  }

  /**
   * Show `view`, mounting it on first use.
   *
   * `sync: true` writes the URL — needed when something other than the nav asks
   * for a view (the nav writes it itself, so its own events don't double up).
   */
  #show(view, { sync = true } = {}) {
    if (view === this.#active) return;
    if (!this.#mounted.has(view) && !this.#pending.has(view)) return;

    if (this.#active) this.#viewEl(this.#active)?.setAttribute("hidden", "");

    const el = this.#mount(view);
    el.removeAttribute("hidden");
    this.#active = view;

    // Reflected so page geometry can depend on which view is showing. Views in
    // one module don't always share a page box — the orchestrator's pinned
    // composer needs a height-capped, clipped host, which would clip the
    // scrolling tables its sibling views are. `:has(> orchestrator-page)` cannot
    // express that: hidden views stay in the DOM, so it would match whichever
    // view is up.
    this.dataset.activeView = view;

    // Keep the nav's highlight in step whether the switch came from a nav click
    // or from `?view=` on load. Setting the attribute is enough: the nav moves
    // one class rather than rebuilding.
    this.querySelector("app-module-nav")?.setAttribute("active-section", view);

    const label = el.dataset.title;
    document.title = label ? `${label} · ${this.#baseTitle}` : this.#baseTitle;

    if (sync) syncView(view);
  }

  #viewEl(view) {
    return this.#mounted.get(view) ?? null;
  }

  /** Instantiate a view's template the first time it is shown; then reuse it. */
  #mount(view) {
    const existing = this.#mounted.get(view);
    if (existing) return existing;

    const template = this.#pending.get(view);
    const fragment = template.content.cloneNode(true);
    const el = fragment.firstElementChild;
    el.dataset.view = view;
    if (template.dataset.title) el.dataset.title = template.dataset.title;
    el.setAttribute("hidden", "");
    template.replaceWith(el);
    this.#pending.delete(view);
    this.#mounted.set(view, el);

    // Deliberately not awaited: the element is in the DOM either way and
    // upgrades when its definition lands. A failed import leaves an empty view
    // rather than taking down the page, so it is worth a console trail.
    const module = template.dataset.module;
    if (module) {
      import(module).catch((e) =>
        console.error(`module-shell: failed to load view "${view}" from ${module}`, e),
      );
    }
    return el;
  }

  /** Programmatic switch (used by in-page links that target a sibling view). */
  show(view) {
    this.#show(view);
  }
}

customElements.define("module-shell", ModuleShell);
