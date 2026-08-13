import { apiFetch } from "/common/services/api.js";
import { icons } from "/common/utils/icons.js";
import { attachSlidingIndicator } from "/common/utils/tab-indicator.js";
import { showToast } from "/common/utils/toast.js";
import { withLoading } from "/common/utils/async-button.js";
import { confirmDialog } from "/common/utils/confirm-dialog.js";
import "/common/components/app-modal.js";
import "/common/components/app-module-nav.js";
import "/common/components/app-empty-state.js";
import "/common/components/app-skeleton.js";

// your-agents-page.css is <link>ed by the host page, not imported here: a sheet
// pulled in by this module only exists once the module does, which is too late
// to style the static shell the page paints before then (see web/your-agents.html).

function statusClass(status) {
  if (status === "running") return "is-running";
  if (status === "error" || status === "failed") return "is-error";
  if (status === "deploying" || status === "starting") return "is-pending";
  return "is-stopped";
}

function parseImageTag(image) {
  if (!image) return { name: "", version: "" };
  const parts = image.split(":");
  return { name: parts[0] || image, version: parts[1] || "latest" };
}

class YourAgentsPage extends HTMLElement {
  #initialized = false;
  #agents = [];
  #statusFilter = "all";
  #sortBy = "name";
  #pollTimer = null;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    // The host page owns the shell (web/your-agents.html) so it paints styled
    // before this module arrives; here we only bind to it and fill the API-fed
    // regions. Fallback for hosts that don't supply it (element created in JS).
    if (!this.querySelector("#agents-grid")) this.insertAdjacentHTML("afterbegin", this.#shell());
    // The deploy dialog stays component-owned: it has no pre-JS footprint, so
    // duplicating it into every host page would buy nothing.
    this.insertAdjacentHTML("beforeend", this.#deployModal());

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

    attachSlidingIndicator(this.querySelector("#status-tabs"), ".type-tab", ".active");
    this.querySelector("#status-tabs").addEventListener("click", (e) => {
      const tab = e.target.closest(".type-tab");
      if (!tab) return;
      this.#statusFilter = tab.dataset.status;
      this.#renderTabs();
      this.#renderGrid();
    });

    this.#setupModal();
    this.#load();
  }

  async #load() {
    const result = await window.fetchContainers("", 1, 100);
    this.#agents = result.data || [];

    // Fetch upload info so we can show upload source (GitHub/Upload) on all
    // agent cards and granular progress for agents still deploying.
    try {
      const res = await apiFetch("/agents/my-uploads");
      if (res.ok) {
        const body = await res.json();
        const uploads = body.data || [];
        const uploadMap = new Map();
        for (const u of uploads) uploadMap.set(u.agent_name, u.upload_info);
        for (const a of this.#agents) {
          a._uploadInfo = uploadMap.get(a.name) || null;
        }
      }
    } catch { /* best-effort */ }

    this.#renderTabs();
    this.#renderGrid();
    this.#schedulePoll();
  }

  #schedulePoll() {
    clearTimeout(this.#pollTimer);
    const hasSettingUp = this.#agents.some(
      (a) => a.status === "deploying" || a.status === "starting",
    );
    if (hasSettingUp) {
      this.#pollTimer = setTimeout(() => this.#pollSettingUp(), 5000);
    }
  }

  async #pollSettingUp() {
    const result = await window.fetchContainers("", 1, 100);
    const freshAgents = result.data || [];
    const freshMap = new Map();
    for (const a of freshAgents) freshMap.set(a.id, a);

    let tabsChanged = false;
    for (const a of this.#agents) {
      if (a.status !== "deploying" && a.status !== "starting") continue;
      const fresh = freshMap.get(a.id);
      if (!fresh || fresh.status === a.status) continue;
      // Status changed — update in place, preserve upload info
      const uploadInfo = a._uploadInfo;
      Object.assign(a, fresh);
      a._uploadInfo = uploadInfo;
      tabsChanged = true;
      // Re-render only this card
      const card = this.querySelector(`[data-agent-id="${a.id}"]`);
      if (card) {
        const tmp = document.createElement("div");
        tmp.innerHTML = this.#renderCard(a);
        card.replaceWith(tmp.firstElementChild);
      }
    }

    if (tabsChanged) this.#renderTabs();
    this.#schedulePoll();
  }

  #renderCard(a) {
    const name = a.display_name || a.name;
    const isRunning = a.status === "running";
    const isError = a.status === "error" || a.status === "failed";
    const isSettingUp = a.status === "deploying" || a.status === "starting";
    const { version: imgVersion } = parseImageTag(a.image);
    const version = a.version || imgVersion;
    const allTags = a.tags || [];
    const shownTags = allTags.slice(0, 2);
    const extraTags = allTags.length - shownTags.length;
    const tagsHtml =
      shownTags.map((t) => `<span class="tag">${this.#esc(t)}</span>`).join("") +
      (extraTags > 0 ? `<span class="tag tag--more">+${extraTags}</span>` : "");

    const cardClass = isError ? " agent-card--error" : isSettingUp ? " agent-card--setting-up" : "";
    const sourceType = a._uploadInfo?.upload_type;
    const sourceLabel = sourceType === "github" ? "GitHub" : sourceType === "zip" ? "Zip" : null;

    let bodyHtml = "";
    if (isError) {
      bodyHtml = `<div class="agent-card-error"><span class="agent-card-error-title">Agent failed</span>Container exited with an error. <a href="/flows.html?agent=${encodeURIComponent(a.id)}" class="error-logs-link">View logs</a></div>`;
    } else if (isSettingUp) {
      const info = a._uploadInfo;
      const statusMsg = info?.status_message || (a.status === "starting" ? "Starting container..." : "Building and deploying...");
      bodyHtml = `
        <div class="agent-card-setup">
          <div class="setup-progress">
            <span class="setup-spinner"></span>
            <span class="setup-label">${this.#esc(statusMsg)}</span>
          </div>
          <p class="setup-hint">This may take a few minutes. Status updates automatically.</p>
        </div>`;
    } else if (a.description) {
      bodyHtml = `<div class="agent-card-desc">${this.#esc(a.description)}</div>`;
    }

    let actionsHtml = "";
    if (isRunning) {
      actionsHtml = `
        <button class="card-action-btn card-action-btn--icon" data-action="restart" data-name="${this.#escAttr(a.name)}" aria-label="Restart ${this.#escAttr(name)}" title="Restart">${icons.refresh("", 14)}</button>
        <button class="card-action-btn card-action-btn--icon" data-action="stop" data-name="${this.#escAttr(a.name)}" aria-label="Stop ${this.#escAttr(name)}" title="Stop">${icons.square("", 12)}</button>`;
    } else if (!isSettingUp) {
      actionsHtml = `
        <button class="card-action-btn card-action-btn--primary" data-action="deploy" data-id="${this.#escAttr(a.id)}" data-name="${this.#escAttr(a.name)}" data-image="${this.#escAttr(a.image || "")}">${icons.play("", 13)} Deploy</button>`;
    }

    return `
    <div class="agent-card${cardClass}" data-agent-id="${this.#escAttr(a.id)}">
      ${sourceLabel ? `<span class="agent-card-source">${sourceLabel}</span>` : ""}
      <div class="agent-card-top">
        <span class="status-dot ${statusClass(a.status)}" title="${this.#esc(a.status)}"></span>
        <a class="agent-card-name" href="/agent-card.html?id=${this.#escAttr(a.id)}">${this.#esc(name)}</a>
        ${version ? `<span class="agent-card-version">v${this.#esc(String(version).replace(/^v/, ""))}</span>` : ""}
      </div>
      ${tagsHtml ? `<div class="agent-card-tags">${tagsHtml}</div>` : ""}
      ${bodyHtml}
      <div class="agent-card-actions">
        ${actionsHtml}
        ${!isSettingUp ? `<button class="card-action-btn card-action-btn--danger" data-action="delete" data-id="${this.#escAttr(a.id)}" data-name="${this.#escAttr(a.name)}" aria-label="Delete ${this.#escAttr(name)}" title="Delete ${this.#escAttr(name)}">
          ${icons.trash("", 14)}
        </button>` : ""}
      </div>
    </div>`;
  }

  disconnectedCallback() {
    clearTimeout(this.#pollTimer);
  }

  #renderTabs() {
    const running = this.#agents.filter((a) => a.status === "running").length;
    const settingUp = this.#agents.filter(
      (a) => a.status === "deploying" || a.status === "starting",
    ).length;
    const failed = this.#agents.filter(
      (a) => a.status === "error" || a.status === "failed",
    ).length;
    const stopped = this.#agents.length - running - settingUp - failed;

    const tab = (key, label, n) =>
      `<button class="type-tab ${this.#statusFilter === key ? "active" : ""}" role="tab"
        aria-selected="${this.#statusFilter === key}" data-status="${key}">
        ${label}<span class="n">${n}</span></button>`;

    this.querySelector("#status-tabs").innerHTML =
      tab("all", "All", this.#agents.length) +
      tab("running", "Running", running) +
      tab("setting-up", "Setting up", settingUp) +
      tab("stopped", "Stopped", stopped) +
      tab("failed", "Failed", failed);
  }

  #updateClearBtn() {
    const input = this.querySelector("#search-input");
    const btn = this.querySelector("#search-clear");
    btn.style.display = input.value ? "" : "none";
  }

  /** Fallback shell — mirrors the static markup in web/your-agents.html. */
  #shell() {
    return `
      <app-module-nav module="agents"></app-module-nav>
      <div class="page-header">
        <div class="page-header-top">
          <div>
            <h1 class="title-page">Your agents</h1>
            <p class="page-desc">Deployed agent containers you manage. Track status and failures, then open one to manage access, versions, and settings.</p>
          </div>
        </div>
      </div>
      <div class="toolbar">
        <div class="search-wrap">
          <span class="search-icon">${icons.search("", 18)}</span>
          <input type="search" id="search-input" placeholder="Search agents by name, skill, or capability..." />
          <button class="search-clear" id="search-clear" aria-label="Clear search" style="display:none">${icons.x("", 16)}</button>
        </div>
        <div class="sort-wrap">
          <span class="sort-icon">${icons.sortBoth("", 16)}</span>
          <select id="sort-select" class="sort-select" aria-label="Sort agents">
            <option value="name">Sort: Name</option>
            <option value="status">Sort: Status</option>
            <option value="version">Sort: Version</option>
          </select>
        </div>
      </div>
      <div class="type-tabs" id="status-tabs" role="tablist">${Array.from({ length: 4 }, () => `<div class="skel-tab"></div>`).join("")}</div>
      <div class="agents-grid" id="agents-grid">${this.#skeletonCards()}</div>
    `;
  }

  #deployModal() {
    return `
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
  }

  #skeletonCards() {
    return Array.from(
      { length: 4 },
      () => `
      <div class="agent-card skeleton-card">
        <div class="skel-line skel-line--name"></div>
        <div class="skel-tags">
          <div class="skel-tag"></div>
          <div class="skel-tag"></div>
        </div>
        <div class="skel-line skel-line--desc"></div>
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

    if (this.#statusFilter !== "all") {
      if (this.#statusFilter === "failed") {
        filtered = filtered.filter(
          (a) => a.status === "error" || a.status === "failed",
        );
      } else if (this.#statusFilter === "running") {
        filtered = filtered.filter((a) => a.status === "running");
      } else if (this.#statusFilter === "setting-up") {
        filtered = filtered.filter(
          (a) => a.status === "deploying" || a.status === "starting",
        );
      } else {
        // "stopped" covers everything that isn't running, setting up, or failed.
        filtered = filtered.filter(
          (a) =>
            a.status !== "running" &&
            a.status !== "deploying" &&
            a.status !== "starting" &&
            a.status !== "error" &&
            a.status !== "failed",
        );
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
            <a href="/add-agent.html" class="empty-action-link empty-action-link--secondary">Import agent</a>
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

    grid.innerHTML = filtered.map((a) => this.#renderCard(a)).join("");
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
        const confirmed = await confirmDialog({
          title: `Delete ${name}`,
          message: `This will stop the container and remove it from the registry. This action cannot be undone.`,
          confirmLabel: 'Delete',
          danger: true,
        });
        if (!confirmed) return;
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
