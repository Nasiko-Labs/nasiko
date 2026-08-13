import { icons } from "/common/utils/icons.js";
import { attachSlidingIndicator } from "/common/utils/tab-indicator.js";
import "/common/components/app-empty-state.js";
import "/common/components/app-skeleton.js";

// agents-page.css is <link>ed by the host page, not imported here: a sheet
// pulled in by this module only exists once the module does, which is too late
// to style the static shell the page paints before then (see web/agents.html).

function statusClass(status) {
  if (status === "running") return "is-running";
  if (status === "error" || status === "failed") return "is-error";
  if (status === "deploying" || status === "starting") return "is-pending";
  return "is-stopped";
}

class AgentsPage extends HTMLElement {
  #initialized = false;
  #agents = [];
  #activeCategory = "all";
  #pinnedTabs = [];

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    // The host page owns the shell (web/agents.html) so it paints styled before
    // this module arrives; here we only bind to it and fill the API-fed regions.
    // Fallback for hosts that don't supply it (e.g. an element created in JS).
    if (!this.querySelector("#agents-grid")) this.insertAdjacentHTML("afterbegin", this.#shell());

    this.querySelector("#search-input").addEventListener("input", () => {
      this.#updateClearBtn();
      this.#renderGrid();
    });

    this.querySelector("#search-clear").addEventListener("click", () => {
      const input = this.querySelector("#search-input");
      input.value = "";
      this.#updateClearBtn();
      this.#renderGrid();
      input.focus();
    });

    // Category tab clicks are delegated — tabs re-render after data loads.
    attachSlidingIndicator(this.querySelector("#category-tabs"), ".type-tab", ".active");
    this.querySelector("#category-tabs").addEventListener("click", (e) => {
      const tab = e.target.closest(".type-tab");
      if (!tab) return;
      this.#activeCategory = tab.dataset.category;
      this.#renderFilter();
      this.#renderGrid();
    });

    // Whole card opens details; explicit links (Details/Chat) keep their own hrefs.
    const openCard = (card) => {
      window.location.href = `/agent-card.html?id=${card.dataset.agentId}`;
    };
    this.querySelector("#agents-grid").addEventListener("click", (e) => {
      if (e.target.closest("a")) return;
      const card = e.target.closest(".card[data-agent-id]");
      if (card) openCard(card);
    });
    this.querySelector("#agents-grid").addEventListener("keydown", (e) => {
      if (e.key !== "Enter" || e.target.closest("a")) return;
      const card = e.target.closest(".card[data-agent-id]");
      if (card) openCard(card);
    });

    this.#loadAgents();
  }

  async #loadAgents() {
    const result = await window.fetchAgents("", 1, 100);
    this.#agents = result.data || [];
    await this.#loadPinnedTabs();
    this.#renderFilter();
    this.#renderGrid();
  }

  /** Admin-pinned tab list (Settings → `catalog_tabs`, comma-separated tags). */
  async #loadPinnedTabs() {
    try {
      const settings = await window.fetchSettings?.();
      this.#pinnedTabs = (settings?.catalog_tabs || "")
        .split(",")
        .map((t) => t.trim().toLowerCase())
        .filter(Boolean);
    } catch {
      this.#pinnedTabs = [];
    }
  }

  #renderFilter() {
    const counts = new Map();
    for (const a of this.#agents) {
      for (const t of a.tags || []) {
        const key = t.toLowerCase();
        counts.set(key, (counts.get(key) || 0) + 1);
      }
    }
    let cats;
    if (this.#pinnedTabs.length) {
      // Admin-pinned tab list (Settings → catalog_tabs) shown as-is, in order.
      cats = this.#pinnedTabs.map((c) => [c, counts.get(c) || 0]);
    } else {
      // Top categories only — every distinct tag as a tab sprawls on big
      // fleets. Long-tail tags stay reachable through search (matches tags).
      cats = [...counts.entries()]
        .sort((x, y) => y[1] - x[1] || x[0].localeCompare(y[0]))
        .slice(0, 5);
    }
    // Keep a selected long-tail category visible while it's active.
    if (this.#activeCategory !== "all" && !cats.some(([c]) => c === this.#activeCategory)) {
      cats.push([this.#activeCategory, counts.get(this.#activeCategory) || 0]);
    }
    const tab = (key, label, n) =>
      `<button class="type-tab ${this.#activeCategory === key ? "active" : ""}" role="tab"
        aria-selected="${this.#activeCategory === key}" data-category="${this.#esc(key)}">
        ${this.#esc(label)}<span class="n">${n}</span></button>`;
    this.querySelector("#category-tabs").innerHTML =
      tab("all", "All", this.#agents.length) +
      cats.map(([c, n]) => tab(c, c.charAt(0).toUpperCase() + c.slice(1), n)).join("");
  }

  #updateClearBtn() {
    const input = this.querySelector("#search-input");
    const btn = this.querySelector("#search-clear");
    btn.style.display = input.value ? "" : "none";
  }

  /** Fallback shell — mirrors the static markup in web/agents.html. */
  #shell() {
    return `
      <div class="page-top">
        <h1 class="title-page">Agent hub</h1>
        <!-- Deliberately count-free: the fleet size lands in the "All N" tab.
             Injecting it here reflowed the description (2 → 3 lines on mobile)
             the moment the API answered. -->
        <p class="subtitle">Discover and chat with the agents deployed on this cluster.</p>
      </div>
      <div class="controls">
        <div class="search-wrap">
          <span class="search-icon">${icons.search("", 18)}</span>
          <input type="search" id="search-input" placeholder="Search agents by name, skill, or capability" />
          <button class="search-clear" id="search-clear" aria-label="Clear search" style="display:none">${icons.x("", 16)}</button>
        </div>
      </div>
      <div class="type-tabs" id="category-tabs" role="tablist">${this.#skeletonTabs()}</div>
      <div class="grid" id="agents-grid">${this.#skeletonCards()}</div>
    `;
  }

  #skeletonTabs() {
    return Array.from({ length: 6 }, () => `<div class="skel-tab"></div>`).join("");
  }

  #skeletonCards() {
    return Array.from(
      { length: 6 },
      () => `
      <div class="card skeleton-card">
        <div class="skel-line skel-line--name"></div>
        <div class="skel-tags">
          <div class="skel-tag"></div>
          <div class="skel-tag"></div>
        </div>
        <div class="skel-line skel-line--desc1"></div>
        <div class="skel-line skel-line--desc2"></div>
      </div>
    `,
    ).join("");
  }

  #renderGrid() {
    const q = (this.querySelector("#search-input")?.value || "").toLowerCase();
    let filtered = this.#agents;

    if (this.#activeCategory !== "all") {
      filtered = filtered.filter((a) =>
        (a.tags || []).some((t) => t.toLowerCase() === this.#activeCategory),
      );
    }
    if (q) {
      filtered = filtered.filter(
        (a) =>
          (a.display_name || a.name || "").toLowerCase().includes(q) ||
          (a.description || "").toLowerCase().includes(q) ||
          (a.tags || []).some((t) => t.toLowerCase().includes(q)),
      );
    }

    const grid = this.querySelector("#agents-grid");
    if (!filtered.length) {
      grid.innerHTML = `
        <div class="empty-wrap">
          <app-empty-state
            title="No agents found"
            description="Try adjusting your search or filter criteria."
            icon='${icons.layers("", 40)}'>
          </app-empty-state>
        </div>`;
      return;
    }

    grid.innerHTML = filtered
      .map((a) => {
        const name = a.display_name || a.name;
        const allTags = a.tags || [];
        const shown = allTags.slice(0, 2);
        const extra = allTags.length - shown.length;
        const tags =
          shown.map((t) => `<span class="tag">${this.#esc(t)}</span>`).join("") +
          (extra > 0 ? `<span class="tag tag--more">+${extra}</span>` : "");
        const version = a.version ? `v${String(a.version).replace(/^v/, "")}` : "";

        return `
        <div class="card" data-agent-id="${encodeURIComponent(a.id)}" role="link" tabindex="0"
          aria-label="Open ${this.#esc(name)} details">
          <div class="card-top">
            ${a.status ? `<span class="status-dot ${statusClass(a.status)}" title="${this.#esc(a.status)}"></span>` : ""}
            <span class="card-name">${this.#esc(name)}</span>
            ${version ? `<span class="card-version">${this.#esc(version)}</span>` : ""}
          </div>
          <div class="card-tags">${tags}</div>
          <div class="card-desc">${this.#esc(a.description || "")}</div>
          <div class="card-foot">
            <a class="card-link" href="/agent-card.html?id=${encodeURIComponent(a.id)}">Details</a>
            <a class="card-chat-btn" href="/chat.html?agent_id=${encodeURIComponent(a.id)}&agent_name=${encodeURIComponent(name)}">Chat ${icons.arrowUpRight("", 13)}</a>
          </div>
        </div>
      `;
      })
      .join("");
  }

  #esc(s) {
    const d = document.createElement("span");
    d.textContent = s || "";
    return d.innerHTML;
  }
}

customElements.define("agents-page", AgentsPage);
