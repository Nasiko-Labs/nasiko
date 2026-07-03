import { icons } from "/common/utils/icons.js";
import styles from './agents-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const MAX_TABS = 5;

function buildCategories(agents) {
  const counts = {};
  const labels = {};
  for (const a of agents) {
    const first = (a.tags || [])[0];
    if (!first) continue;
    const key = first.toLowerCase();
    counts[key] = (counts[key] || 0) + 1;
    if (!labels[key]) labels[key] = first.charAt(0).toUpperCase() + first.slice(1);
  }
  const sorted = Object.keys(counts).sort((a, b) => counts[b] - counts[a]);
  const visible = sorted.slice(0, MAX_TABS).map((key) => ({ key, label: labels[key] }));
  if (sorted.length > MAX_TABS) visible.push({ key: "_misc", label: "More" });
  return [{ key: "all", label: "All" }, ...visible];
}

class AgentsPage extends HTMLElement {
  #agents = [];
  #activeCategory = "all";

  connectedCallback() {
    this.innerHTML = `
      <h1 class="title">Choose an agent to assist you</h1>
      <div class="search-wrap">
        <span class="icon">${icons.search("", 20)}</span>
        <input type="search" placeholder="Search agents by name, skill, or capability..." />
      </div>
      <nav class="tabs"><button class="tab is-active" data-cat="all"><span>All</span></button></nav>
      <div class="grid">${this.#skeletonCards()}</div>
    `;

    this.querySelector(".tabs").addEventListener("click", (e) => {
      const tab = e.target.closest(".tab");
      if (!tab) return;
      this.#activeCategory = tab.dataset.cat;
      this.querySelectorAll(".tab").forEach((t) =>
        t.classList.remove("is-active"),
      );
      tab.classList.add("is-active");
      this.#renderGrid();
    });

    this.querySelector("input").addEventListener("input", () =>
      this.#renderGrid(),
    );

    this.#loadAgents();
  }

  async #loadAgents() {
    const result = await window.fetchAgents("", 1, 100);
    this.#agents = result.data || [];
    this.#renderTabs();
    this.#renderGrid();
  }

  #renderTabs() {
    const cats = buildCategories(this.#agents);
    this.querySelector(".tabs").innerHTML = cats
      .map(
        (c) =>
          `<button class="tab${c.key === this.#activeCategory ? " is-active" : ""}" data-cat="${c.key}"><span>${c.label}</span></button>`,
      )
      .join("");
  }

  #skeletonCards() {
    return Array.from(
      { length: 6 },
      () => `
      <div class="card" style="min-height:160px;">
        <div style="width:60%;height:1em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
        <div style="display:flex;gap:6px;margin-top:var(--space-sm);">
          <div style="width:50px;height:1.2em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
          <div style="width:60px;height:1.2em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
          <div style="width:70px;height:1.2em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
        </div>
        <div style="width:90%;height:0.8em;background:var(--color-border);border-radius:var(--radius-sm);margin-top:var(--space-sm);"></div>
        <div style="width:70%;height:0.8em;background:var(--color-border);border-radius:var(--radius-sm);margin-top:4px;"></div>
        <div style="width:100%;height:2.2em;background:var(--color-border);border-radius:var(--radius-md);margin-top:auto;opacity:0.5;"></div>
      </div>
    `,
    ).join("");
  }

  #renderGrid() {
    const q = this.querySelector("input").value.toLowerCase();
    let filtered = this.#agents;

    if (this.#activeCategory !== "all") {
      const cats = buildCategories(this.#agents);
      const topKeys = new Set(cats.filter((c) => c.key !== "all" && c.key !== "_misc").map((c) => c.key));
      if (this.#activeCategory === "_misc") {
        filtered = filtered.filter((a) => {
          const first = ((a.tags || [])[0] || "").toLowerCase();
          return first && !topKeys.has(first);
        });
      } else {
        filtered = filtered.filter(
          (a) => ((a.tags || [])[0] || "").toLowerCase() === this.#activeCategory,
        );
      }
    }
    if (q) {
      filtered = filtered.filter(
        (a) =>
          (a.display_name || a.name || "").toLowerCase().includes(q) ||
          (a.description || "").toLowerCase().includes(q) ||
          (a.tags || []).some((t) => t.toLowerCase().includes(q)),
      );
    }

    const grid = this.querySelector(".grid");
    if (!filtered.length) {
      grid.innerHTML = '<div class="empty">No agents found.</div>';
      return;
    }

    grid.innerHTML = filtered
      .map(
        (a) => `
      <div class="card">
        <div class="card-name">${a.display_name || a.name}</div>
        <div class="card-tags">${(a.tags || []).map((t) => `<span class="tag">${t}</span>`).join("")}</div>
        <div class="card-desc">${a.description || ""}</div>
        <a class="card-btn" href="/agent-card.html?id=${a.id}">View agent</a>
      </div>
    `,
      )
      .join("");
  }
}

customElements.define("agents-page", AgentsPage);
