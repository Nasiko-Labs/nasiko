import { icons } from "/common/utils/icons.js";
import "/common/components/app-badge.js";
import "/common/components/app-empty-state.js";
import "/common/components/app-skeleton.js";

import styles from "./agents-page.css" with { type: "css" };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const AVATAR_COLORS = [
  "var(--color-primary)",
  "var(--color-success)",
  "var(--color-warning)",
  "var(--color-error)",
  "var(--color-info)",
  "var(--color-neutral)",
];

function hashStr(str) {
  let h = 0;
  for (let i = 0; i < str.length; i++) {
    h = ((h << 5) - h + str.charCodeAt(i)) | 0;
  }
  return Math.abs(h);
}

function avatarColor(name) {
  return AVATAR_COLORS[hashStr(name) % AVATAR_COLORS.length];
}

function avatarLetter(agent) {
  const name = agent.display_name || agent.name || "?";
  return name.charAt(0).toUpperCase();
}

function statusVariant(status) {
  if (status === "running") return "success";
  if (status === "stopped") return "warning";
  if (status === "error" || status === "failed") return "error";
  if (status === "deploying" || status === "starting") return "info";
  return "neutral";
}

function collectCategories(agents) {
  const cats = new Set();
  for (const a of agents) {
    for (const t of a.tags || []) {
      cats.add(t.toLowerCase());
    }
  }
  return [...cats].sort();
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
      <div class="controls">
        <div class="search-wrap">
          <span class="search-icon">${icons.search("", 18)}</span>
          <input type="search" id="search-input" placeholder="Search agents by name, skill, or capability..." />
          <button class="search-clear" id="search-clear" aria-label="Clear search" style="display:none">${icons.x("", 16)}</button>
        </div>
        <div class="filter-wrap">
          <label class="filter-label" for="category-filter">Category</label>
          <select id="category-filter" class="filter-select">
            <option value="all">All categories</option>
          </select>
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

    this.querySelector("#category-filter").addEventListener("change", (e) => {
      this.#activeCategory = e.target.value;
      this.#renderGrid();
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
    const cats = collectCategories(this.#agents);
    const select = this.querySelector("#category-filter");
    select.innerHTML =
      `<option value="all">All categories</option>` +
      cats.map((c) => `<option value="${c}">${c.charAt(0).toUpperCase() + c.slice(1)}</option>`).join("");
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
        <div class="card-header">
          <div class="skel-avatar"></div>
          <div class="skel-badge"></div>
        </div>
        <div class="skel-line skel-line--name"></div>
        <div class="skel-tags">
          <div class="skel-tag"></div>
          <div class="skel-tag"></div>
        </div>
        <div class="skel-line skel-line--desc1"></div>
        <div class="skel-line skel-line--desc2"></div>
        <div class="skel-actions">
          <div class="skel-btn"></div>
          <div class="skel-btn"></div>
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
        const letter = avatarLetter(a);
        const color = avatarColor(name);
        const variant = statusVariant(a.status);
        const tags = (a.tags || [])
          .map((t) => `<app-badge variant="neutral">${this.#esc(t)}</app-badge>`)
          .join("");

        return `
        <div class="card">
          <div class="card-header">
            <div class="avatar" style="background:color-mix(in srgb, ${color} 15%, transparent);color:${color}">
              ${letter}
            </div>
            ${a.status ? `<app-badge variant="${variant}" dot>${a.status}</app-badge>` : ""}
          </div>
          <div class="card-name">${this.#esc(name)}</div>
          <div class="card-tags">${tags}</div>
          <div class="card-desc">${this.#esc(a.description || "")}</div>
          <div class="card-actions">
            <a class="btn-chat" href="/chat.html?agent_id=${encodeURIComponent(a.id)}&agent_name=${encodeURIComponent(name)}">${icons.send("", 14)} Chat</a>
            <a class="btn-view" href="/agent-card.html?id=${encodeURIComponent(a.id)}">View agent</a>
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
