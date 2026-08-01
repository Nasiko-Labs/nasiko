import { icons } from "/common/utils/icons.js";
import "/common/components/app-empty-state.js";
import "/common/components/app-skeleton.js";

import styles from "./agents-page.css" with { type: "css" };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

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

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <div class="page-top">
        <h1 class="title">Agent Catalog</h1>
        <p class="subtitle" id="agent-count"></p>
      </div>
      <div class="type-tabs" id="category-tabs" role="tablist"></div>
      <div class="controls">
        <div class="search-wrap">
          <span class="search-icon">${icons.search("", 18)}</span>
          <input type="search" id="search-input" placeholder="Search agents by name, skill, or capability..." />
          <button class="search-clear" id="search-clear" aria-label="Clear search" style="display:none">${icons.x("", 16)}</button>
        </div>
      </div>
      <div class="grid" id="agents-grid">${this.#skeletonCards()}</div>
    `;

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
    this.#renderCount();
    this.#renderFilter();
    this.#renderGrid();
  }

  #renderCount() {
    const el = this.querySelector("#agent-count");
    if (el) el.textContent = `${this.#agents.length} agent${this.#agents.length !== 1 ? "s" : ""} available`;
  }

  #renderFilter() {
    const counts = new Map();
    for (const a of this.#agents) {
      for (const t of a.tags || []) {
        counts.set(t, (counts.get(t) || 0) + 1);
      }
    }
    const cats = [...counts.entries()].sort((x, y) => y[1] - x[1]);
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

  #skeletonCards() {
    return Array.from(
      { length: 6 },
      () => `
      <div class="card skeleton-card">
        <div class="card-top">
          <div class="skel-line skel-line--name"></div>
          <div class="skel-dot"></div>
        </div>
        <div class="skel-line skel-line--desc1"></div>
        <div class="skel-line skel-line--desc2"></div>
        <div class="skel-tags">
          <div class="skel-tag"></div>
          <div class="skel-tag"></div>
        </div>
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
        const tags = (a.tags || [])
          .slice(0, 3)
          .map((t) => `<span class="tag">${this.#esc(t)}</span>`)
          .join("");

        return `
        <div class="card" data-agent-id="${encodeURIComponent(a.id)}" role="link" tabindex="0"
          aria-label="Open ${this.#esc(name)} details">
          <div class="card-top">
            <span class="card-name">${this.#esc(name)}</span>
            ${a.status ? `<span class="card-status ${statusClass(a.status)}"><span class="status-dot"></span>${this.#esc(a.status)}</span>` : ""}
          </div>
          <div class="card-desc">${this.#esc(a.description || "")}</div>
          <div class="card-foot">
            <div class="card-tags">${tags}</div>
            <div class="card-actions">
              <a class="card-link" href="/agent-card.html?id=${encodeURIComponent(a.id)}">Details</a>
              <a class="card-link card-link--primary" href="/chat.html?agent_id=${encodeURIComponent(a.id)}&agent_name=${encodeURIComponent(name)}">${icons.send("", 13)} Chat</a>
            </div>
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
