import { apiFetch } from "/common/services/api.js";
import { icons } from "/common/utils/icons.js";
import { showToast } from "/common/utils/toast.js";
import { withLoading } from "/common/utils/async-button.js";
import "/common/components/app-modal.js";
import "/common/components/app-badge.js";
import "/common/components/app-empty-state.js";
import "/common/components/app-skeleton.js";

import styles from "./your-agents-page.css" with { type: "css" };
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

function parseImageTag(image) {
  if (!image) return { name: "", version: "" };
  const parts = image.split(":");
  return { name: parts[0] || image, version: parts[1] || "latest" };
}

class YourAgentsPage extends HTMLElement {
  #initialized = false;
  #agents = [];
  #statusFilter = null;
  #sortBy = "name";

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <div class="page-header">
        <div class="page-header-top">
          <div>
            <h1 class="page-title">Your Agents</h1>
            <p class="page-desc">Deployed agent containers you manage.</p>
          </div>
        </div>
        <div class="stats-bar" id="stats-bar"></div>
      </div>
      <div class="toolbar">
        <div class="search-wrap">
          <span class="search-icon">${icons.search("", 18)}</span>
          <input type="search" id="search-input" placeholder="Search agents..." />
          <button class="search-clear" id="search-clear" aria-label="Clear search" style="display:none">${icons.x("", 16)}</button>
        </div>
        <div class="sort-wrap">
          <label class="sort-label" for="sort-select">Sort by</label>
          <select id="sort-select" class="sort-select">
            <option value="name">Name</option>
            <option value="status">Status</option>
            <option value="version">Version</option>
          </select>
        </div>
      </div>
      <div class="agents-grid" id="agents-grid">
        ${this.#skeletonCards()}
      </div>

      <app-modal id="deploy-modal" heading="Deploy Agent">
        <div class="modal-section">
          <h3>Environment Variables</h3>
          <p>These will be injected into the container. Saved to agent secrets for future deploys.</p>
          <div id="env-rows"></div>
          <div style="display:flex;gap:var(--space-sm);margin-top:var(--space-xs);">
            <button class="add-env-btn" id="btn-add-env">${icons.plus("", 14)} Add variable</button>
            <button class="import-btn" id="btn-import-secrets">${icons.key("", 14)} Import from secrets</button>
          </div>
        </div>
        <div class="modal-section" id="secrets-import-section" style="display:none;">
          <h3>Select secrets to import</h3>
          <div class="secret-chips" id="secret-chips"></div>
        </div>
        <div class="form-actions">
          <button class="btn-cancel" id="deploy-cancel">Cancel</button>
          <button class="btn-deploy" id="deploy-confirm">Deploy</button>
        </div>
      </app-modal>
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

    this.querySelector("#sort-select").addEventListener("change", (e) => {
      this.#sortBy = e.target.value;
      this.#renderGrid();
    });

    this.querySelector("#stats-bar").addEventListener("click", (e) => {
      const chip = e.target.closest(".stat-chip[data-filter]");
      if (!chip) return;
      const filter = chip.dataset.filter;
      if (this.#statusFilter === filter) {
        this.#statusFilter = null;
      } else {
        this.#statusFilter = filter;
      }
      this.#renderStats();
      this.#renderGrid();
    });

    this.#setupModal();
    this.#load();
  }

  async #load() {
    const result = await window.fetchContainers("", 1, 100);
    this.#agents = result.data || [];
    this.#renderStats();
    this.#renderGrid();
  }

  #renderStats() {
    const running = this.#agents.filter((a) => a.status === "running").length;
    const stopped = this.#agents.filter((a) => a.status === "stopped").length;
    const errored = this.#agents.filter(
      (a) => a.status === "error" || a.status === "failed",
    ).length;

    const chip = (filter, variant, count, label) => {
      const active = this.#statusFilter === filter ? " stat-chip--active" : "";
      return `<button class="stat-chip${active}" data-filter="${filter}"><app-badge variant="${variant}" dot>${count} ${label}</app-badge></button>`;
    };

    this.querySelector("#stats-bar").innerHTML = `
      ${chip("running", "success", running, "running")}
      ${chip("stopped", "warning", stopped, "stopped")}
      ${errored ? chip("error", "error", errored, "error") : ""}
      <span class="stat-total">${this.#agents.length} total</span>
    `;
  }

  #updateClearBtn() {
    const input = this.querySelector("#search-input");
    const btn = this.querySelector("#search-clear");
    btn.style.display = input.value ? "" : "none";
  }

  #skeletonCards() {
    return Array.from(
      { length: 4 },
      () => `
      <div class="agent-card skeleton-card">
        <div class="agent-card-header">
          <div class="skel-avatar"></div>
          <div class="skel-badge"></div>
        </div>
        <div class="skel-line skel-line--name"></div>
        <div class="skel-meta">
          <div class="skel-line skel-line--image"></div>
          <div class="skel-line skel-line--version"></div>
        </div>
        <div class="skel-actions">
          <div class="skel-btn"></div>
          <div class="skel-btn"></div>
          <div class="skel-btn skel-btn--sm"></div>
        </div>
      </div>
    `,
    ).join("");
  }

  #renderGrid() {
    const q = (this.querySelector("#search-input")?.value || "").toLowerCase();
    let filtered = this.#agents;

    if (this.#statusFilter) {
      if (this.#statusFilter === "error") {
        filtered = filtered.filter(
          (a) => a.status === "error" || a.status === "failed",
        );
      } else {
        filtered = filtered.filter((a) => a.status === this.#statusFilter);
      }
    }

    if (q) {
      filtered = filtered.filter(
        (a) =>
          (a.display_name || a.name || "").toLowerCase().includes(q) ||
          (a.image || "").toLowerCase().includes(q),
      );
    }

    filtered = this.#sortAgents(filtered);

    const grid = this.querySelector("#agents-grid");
    if (!this.#agents.length) {
      grid.innerHTML = `
        <div class="empty-wrap">
          <app-empty-state
            title="No agents deployed"
            description="Deploy your first agent from the catalog or add a new one."
            icon='${icons.layers("", 40)}'>
            <a href="/agents.html" class="empty-action-link">Browse catalog</a>
            <a href="/add-agent.html" class="empty-action-link empty-action-link--secondary">Add agent</a>
          </app-empty-state>
        </div>`;
      return;
    }

    if (!filtered.length) {
      grid.innerHTML = `
        <div class="empty-wrap">
          <app-empty-state
            title="No matching agents"
            description="Try adjusting your search or filter criteria."
            icon='${icons.search("", 40)}'>
          </app-empty-state>
        </div>`;
      return;
    }

    grid.innerHTML = filtered
      .map((a) => {
        const name = a.display_name || a.name;
        const letter = avatarLetter(a);
        const color = avatarColor(name);
        const isRunning = a.status === "running";
        const isError = a.status === "error" || a.status === "failed";
        const variant = statusVariant(a.status);
        const { name: imgName, version } = parseImageTag(a.image);

        return `
        <div class="agent-card${isError ? " agent-card--error" : ""}">
          <div class="agent-card-header">
            <div class="agent-avatar" style="background:color-mix(in srgb, ${color} 15%, transparent);color:${color}">
              ${letter}
            </div>
            <app-badge variant="${variant}" dot>${a.status}</app-badge>
          </div>
          <a class="agent-card-name" href="/agent-card.html?id=${this.#escAttr(a.id)}">${this.#esc(name)}</a>
          <div class="agent-card-meta">
            <code class="agent-card-image">${this.#esc(imgName)}</code>
            ${version ? `<app-badge variant="neutral">${this.#esc(version)}</app-badge>` : ""}
          </div>
          ${isError ? `<div class="agent-card-error">Container exited with an error. <a href="/flows.html?agent=${encodeURIComponent(a.id)}" class="error-logs-link">View logs</a></div>` : ""}
          <div class="agent-card-actions">
            ${
              isRunning
                ? `
              <button class="card-action-btn" data-action="restart" data-name="${this.#escAttr(a.name)}" aria-label="Restart ${this.#escAttr(name)}">${icons.refresh("", 14)} Restart</button>
              <button class="card-action-btn" data-action="stop" data-name="${this.#escAttr(a.name)}" aria-label="Stop ${this.#escAttr(name)}">${icons.square("", 12)} Stop</button>
            `
                : `
              <button class="card-action-btn card-action-btn--primary" data-action="deploy" data-id="${this.#escAttr(a.id)}" data-name="${this.#escAttr(a.name)}" data-image="${this.#escAttr(a.image || "")}">Deploy</button>
            `
            }
            <button class="card-action-btn card-action-btn--danger" data-action="delete" data-id="${this.#escAttr(a.id)}" data-name="${this.#escAttr(a.name)}" aria-label="Delete ${this.#escAttr(name)}">
              ${icons.trash("", 14)} Delete
            </button>
          </div>
        </div>
      `;
      })
      .join("");
  }

  #sortAgents(agents) {
    const copy = [...agents];
    if (this.#sortBy === "name") {
      copy.sort((a, b) =>
        (a.display_name || a.name || "").localeCompare(
          b.display_name || b.name || "",
        ),
      );
    } else if (this.#sortBy === "status") {
      const order = { running: 0, deploying: 1, starting: 2, stopped: 3, error: 4, failed: 5 };
      copy.sort(
        (a, b) => (order[a.status] ?? 9) - (order[b.status] ?? 9),
      );
    } else if (this.#sortBy === "version") {
      copy.sort((a, b) =>
        (a.version || "").localeCompare(b.version || ""),
      );
    }
    return copy;
  }

  #setupModal() {
    let deployAgentId = null;
    let deployAgentName = null;
    let deployImage = null;
    let userSecrets = [];
    let selectedSecrets = new Set();

    const modal = this.querySelector("#deploy-modal");
    const envRows = this.querySelector("#env-rows");
    const secretsSection = this.querySelector("#secrets-import-section");
    const secretChips = this.querySelector("#secret-chips");

    const addEnvRow = (key = "", value = "") => {
      const row = document.createElement("div");
      row.className = "env-row";
      row.innerHTML = `<input type="text" placeholder="KEY" value="${this.#escAttr(key)}" /><input type="text" placeholder="value" value="${this.#escAttr(value)}" /><button class="env-remove" aria-label="Remove variable">${icons.xCircle("", 16)}</button>`;
      row.querySelector(".env-remove").addEventListener("click", () => row.remove());
      envRows.appendChild(row);
    };

    this.querySelector("#btn-add-env").addEventListener("click", () =>
      addEnvRow(),
    );

    this.querySelector("#btn-import-secrets").addEventListener(
      "click",
      async () => {
        if (secretsSection.style.display !== "none") {
          secretsSection.style.display = "none";
          return;
        }
        try {
          const res = await apiFetch("/secrets");
          if (!res.ok) throw new Error();
          userSecrets = await res.json();
        } catch {
          userSecrets = [];
        }

        if (!userSecrets.length) {
          showToast("No user secrets found. Add them in Settings.");
          return;
        }

        secretChips.innerHTML = userSecrets
          .map(
            (s) =>
              `<span class="secret-chip" data-name="${this.#escAttr(s.name)}">${this.#esc(s.name)}</span>`,
          )
          .join("");
        secretsSection.style.display = "";

        secretChips.querySelectorAll(".secret-chip").forEach((chip) => {
          chip.addEventListener("click", () => {
            const name = chip.dataset.name;
            if (selectedSecrets.has(name)) {
              selectedSecrets.delete(name);
              chip.classList.remove("selected");
            } else {
              selectedSecrets.add(name);
              chip.classList.add("selected");
            }
          });
        });
      },
    );

    this.querySelector("#deploy-cancel").addEventListener("click", () =>
      modal.close(),
    );

    const deployBtn = this.querySelector("#deploy-confirm");
    deployBtn.addEventListener(
      "click",
      withLoading(deployBtn, "Deploying...", async () => {
        if (selectedSecrets.size > 0 && deployAgentId) {
          await apiFetch(
            `/agents/${deployAgentId}/secrets/import`,
            {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ secret_names: [...selectedSecrets] }),
            },
          );
        }

        const rows = envRows.querySelectorAll(".env-row");
        for (const row of rows) {
          const inputs = row.querySelectorAll("input");
          const key = inputs[0].value.trim();
          const val = inputs[1].value;
          if (!key) continue;
          await apiFetch(`/agents/${deployAgentId}/secrets`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ name: key, value: val }),
          });
        }

        const res = await apiFetch("/containers", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            image: deployImage,
            name: deployAgentName,
          }),
        });
        if (!res.ok) throw new Error(await res.text());

        modal.close();
        this.#load();
        showToast(`Deployed ${deployAgentName}`);
      }),
    );

    this.addEventListener("click", async (e) => {
      const btn = e.target.closest("[data-action]");
      if (!btn) return;
      const { action } = btn.dataset;

      if (action === "deploy") {
        deployAgentId = btn.dataset.id;
        deployAgentName = btn.dataset.name;
        deployImage = btn.dataset.image;
        envRows.innerHTML = "";
        selectedSecrets.clear();
        secretsSection.style.display = "none";

        try {
          const res = await apiFetch(
            `/agents/${deployAgentId}/secrets`,
          );
          if (res.ok) {
            const secrets = await res.json();
            if (secrets.length) {
              for (const s of secrets) addEnvRow(s.name, "");
            }
          }
        } catch {
          /* no secrets */
        }

        modal.setAttribute("heading", `Deploy ${deployAgentName}`);
        modal.open();
      } else if (action === "restart" || action === "stop") {
        const name = btn.dataset.name;
        const original = btn.innerHTML;
        btn.disabled = true;
        btn.textContent = action === "restart" ? "Restarting..." : "Stopping...";
        try {
          const res = await apiFetch(
            `/containers/${encodeURIComponent(name)}/${action}`,
            { method: "POST" },
          );
          if (!res.ok) throw new Error(await res.text());
          showToast(
            `${action === "restart" ? "Restarted" : "Stopped"} ${name}`,
          );
          this.#load();
        } catch (err) {
          showToast(`Failed to ${action}: ${err.message}`);
          btn.disabled = false;
          btn.innerHTML = original;
        }
      } else if (action === "delete") {
        const name = btn.dataset.name;
        const id = btn.dataset.id;
        if (
          !confirm(
            `Delete agent "${name}"? This will stop the container and remove it from the registry.`,
          )
        )
          return;
        try {
          const res = await apiFetch(
            `/agents/${encodeURIComponent(id)}`,
            { method: "DELETE" },
          );
          if (!res.ok) throw new Error(await res.text());
          this.#load();
          showToast(`Deleted ${name}`);
        } catch (err) {
          showToast(`Failed to delete: ${err.message}`);
        }
      }
    });
  }

  #esc(s) {
    const d = document.createElement("span");
    d.textContent = s || "";
    return d.innerHTML;
  }

  #escAttr(s) {
    return (s || "")
      .replace(/&/g, "&amp;")
      .replace(/"/g, "&quot;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }
}

customElements.define("your-agents-page", YourAgentsPage);
