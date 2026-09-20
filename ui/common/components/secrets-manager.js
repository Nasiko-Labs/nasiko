import { icons } from '/common/utils/icons.js';
import { apiFetch, fetchApi } from '/common/services/api.js';
import { timeAgo } from '/common/utils/date-utils.js';
import '/common/components/app-skeleton.js';

import styles from './secrets-manager.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

/**
 * <secrets-manager> — the single secrets surface for every scope.
 *
 * Lists secret names (values are write-only and never returned by the API),
 * adds/replaces one from an inline row, and deletes one behind an inline
 * confirm. No modals, no alert()/confirm().
 *
 * Usage:
 *   <secrets-manager scope="user" heading="Secrets" description="..."></secrets-manager>
 *
 *   const el = document.createElement('secrets-manager');
 *   el.setAttribute('scope', 'agent');
 *   el.agentId = '<uuid>';
 *
 * Attributes:
 *   scope="user|agent"  which API backs it (default "user")
 *   agent-id="<uuid>"   required for scope="agent" (or set `.agentId`)
 *   heading="..."       section heading; omit for a headless embed
 *   description="..."   sub-line under the heading
 *   readonly            read-only mode — no add row, no delete buttons
 *   defer               don't fetch on connect; the host calls .refresh()
 *
 * Events:
 *   secrets-changed  {detail:{scope, action:'add'|'remove', name}} after a mutation
 */

/** Human copy for the empty state, per scope. */
const EMPTY_COPY = {
  user: 'No secrets yet. Add one below — agents and router configs reference it by name.',
  agent: 'No secrets configured for this agent yet. Add one below.',
};

/** Reason a list request can fail with no rows to show. */
const FORBIDDEN_COPY = {
  user: 'Your session cannot read these secrets.',
  agent: 'You need owner access to manage this agent’s secrets.',
};

/**
 * Scope adapters — the one and only place the two secret APIs differ.
 *
 * Everything below this object is scope-agnostic and talks to `{list, add,
 * remove}` alone.
 *
 * user  (oss/server/src/secrets/routes.rs:19-25)
 *   GET    /api/secrets            -> ApiResponse { data: [{id,name,created_at,updated_at}] }
 *   POST   /api/secrets            {name,value} — upsert (ON CONFLICT DO UPDATE)
 *   DELETE /api/secrets/{name}
 * agent (oss/server/src/catalog/agent_secrets.rs:18-31)
 *   GET    /api/agents/{id}/secrets        -> bare [{name, updated_at}] (no envelope)
 *   POST   /api/agents/{id}/secrets        {name,value} — upsert (jsonb_set)
 *   DELETE /api/agents/{id}/secrets/{name}
 */
const SCOPE_ADAPTERS = {
  user: () => ({
    list: async () => {
      const body = await fetchApi('/secrets');
      return toEntries(body?.data ?? body);
    },
    // POST upserts, so it covers both "add" and "replace"; PUT /secrets/{name}
    // is update-only (404 when absent) and would need a second round trip.
    add: (name, value) => sendJson('/secrets', 'POST', { name, value }),
    remove: (name) => sendJson(`/secrets/${encodeURIComponent(name)}`, 'DELETE'),
  }),

  agent: (agentId) => {
    const base = `/agents/${encodeURIComponent(agentId)}/secrets`;
    return {
      list: async () => toEntries(await fetchApi(base)),
      add: (name, value) => sendJson(base, 'POST', { name, value }),
      remove: (name) => sendJson(`${base}/${encodeURIComponent(name)}`, 'DELETE'),
    };
  },
};

/** Mutating call that returns no body worth parsing; throws the server's text. */
async function sendJson(path, method, body) {
  const opts = { method };
  if (body) {
    opts.headers = { 'Content-Type': 'application/json' };
    opts.body = JSON.stringify(body);
  }
  const res = await apiFetch(path, opts);
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(text.trim() || `HTTP ${res.status}`);
  }
}

/** Normalize either list shape into `{name, updatedAt}` rows, sorted by name. */
function toEntries(list) {
  return (Array.isArray(list) ? list : [])
    .map((s) => ({ name: s?.name || '', updatedAt: s?.updated_at || s?.created_at || null }))
    .filter((s) => s.name)
    .sort((a, b) => a.name.localeCompare(b.name));
}

const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) =>
  ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

class SecretsManager extends HTMLElement {
  #initialized = false;
  #adapter = null;
  #secrets = [];
  #status = 'loading';   // 'loading' | 'ready' | 'denied'
  #pendingDelete = null; // name awaiting inline confirm
  #busy = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.#renderShell();
    if (!this.hasAttribute('defer')) this.refresh();
  }

  set agentId(value) { this.setAttribute('agent-id', value ?? ''); }

  get agentId() { return this.getAttribute('agent-id') || ''; }

  get scope() { return this.getAttribute('scope') === 'agent' ? 'agent' : 'user'; }

  get readOnly() { return this.hasAttribute('readonly'); }

  /** Public: (re)load the list. Safe to call repeatedly; hosts use it to lazy-load. */
  async refresh() {
    if (!this.#initialized) return;
    this.#pendingDelete = null;
    try {
      this.#secrets = await this.#scopeAdapter().list();
      this.#status = 'ready';
    } catch {
      this.#secrets = [];
      this.#status = 'denied';
    }
    this.#renderList();
    this.#syncAddRow();
  }

  /* ── Rendering ────────────────────────────────────────────────────────── */

  #renderShell() {
    const heading = this.getAttribute('heading');
    const description = this.getAttribute('description');
    this.innerHTML = `
      ${heading ? `<h2 class="sm-title">${esc(heading)}</h2>` : ''}
      ${description ? `<p class="sm-sub">${esc(description)}</p>` : ''}
      <div class="sm-list" id="sm-list"><app-skeleton lines="3" height="88px"></app-skeleton></div>
      <form class="sm-add" id="sm-add" hidden>
        <label class="sm-field">
          <span class="sm-field-label">Name</span>
          <input type="text" id="sm-name" class="sm-name-input" placeholder="API_KEY"
            pattern="[A-Z_][A-Z0-9_]*" maxlength="128" autocomplete="off" spellcheck="false"
            title="Uppercase letters, digits and underscore; must start with a letter or underscore." required />
        </label>
        <label class="sm-field">
          <span class="sm-field-label">Value</span>
          <input type="password" id="sm-value" placeholder="sk-…" autocomplete="off" required />
        </label>
        <button type="submit" class="sm-btn sm-btn--primary" id="sm-submit">
          ${icons.plus('', 14)} Add secret
        </button>
      </form>
      <p class="sm-msg" id="sm-msg" role="status" hidden></p>`;

    this.querySelector('#sm-add')?.addEventListener('submit', (e) => this.#onAdd(e));
    this.querySelector('#sm-list')?.addEventListener('click', (e) => this.#onListClick(e));
  }

  #renderList() {
    const list = this.querySelector('#sm-list');
    if (!list) return;
    if (this.#status === 'denied') {
      list.innerHTML = `<p class="sm-note">${FORBIDDEN_COPY[this.scope]}</p>`;
      return;
    }
    if (!this.#secrets.length) {
      list.innerHTML = `
        <div class="sm-empty">
          ${icons.lock('sm-empty-icon', 16)}
          <span>${EMPTY_COPY[this.scope]}</span>
        </div>`;
      return;
    }
    list.innerHTML = `<ul class="sm-rows">${this.#secrets.map((s) => this.#rowHtml(s)).join('')}</ul>`;
  }

  #rowHtml(secret) {
    const name = esc(secret.name);
    if (this.#pendingDelete === secret.name) {
      return `
        <li class="sm-row is-confirming">
          <span class="sm-name">${icons.lock('', 13)} ${name}</span>
          <span class="sm-confirm-text">Delete this secret?</span>
          <button type="button" class="sm-btn sm-btn--quiet" data-cancel>Cancel</button>
          <button type="button" class="sm-btn sm-btn--danger" data-confirm="${name}">Delete</button>
        </li>`;
    }
    return `
      <li class="sm-row">
        <span class="sm-name">${icons.lock('', 13)} ${name}</span>
        <span class="sm-value">••••••••</span>
        <span class="sm-meta">${secret.updatedAt ? `Updated ${esc(timeAgo(secret.updatedAt))}` : ''}</span>
        ${this.readOnly ? '' : `
        <button type="button" class="sm-delete" data-delete="${name}"
          aria-label="Delete secret ${name}">${icons.trash('', 14)}</button>`}
      </li>`;
  }

  /** The add row and its read-only note follow permission + load state. */
  #syncAddRow() {
    const form = this.querySelector('#sm-add');
    if (!form) return;
    form.hidden = this.readOnly || this.#status === 'denied';
  }

  #showMessage(text, tone) {
    const msg = this.querySelector('#sm-msg');
    if (!msg) return;
    msg.textContent = text;
    msg.classList.toggle('is-error', tone === 'error');
    msg.classList.toggle('is-ok', tone === 'ok');
    msg.hidden = !text;
  }

  /* ── Mutations ────────────────────────────────────────────────────────── */

  async #onAdd(e) {
    e.preventDefault();
    if (this.#busy) return;
    const nameInput = this.querySelector('#sm-name');
    const valueInput = this.querySelector('#sm-value');
    const name = nameInput.value.trim();
    if (!name || !valueInput.value) return;

    this.#setBusy(true);
    try {
      await this.#scopeAdapter().add(name, valueInput.value);
    } catch (err) {
      this.#showMessage(`Could not save ${name}: ${err.message}`, 'error');
      this.#setBusy(false);
      return;
    }
    nameInput.value = '';
    valueInput.value = '';
    this.#setBusy(false);
    this.#showMessage(this.#savedCopy(name), 'ok');
    this.#emitChanged('add', name);
    await this.refresh();
  }

  async #onListClick(e) {
    const deleteBtn = e.target.closest('[data-delete]');
    if (deleteBtn) {
      this.#pendingDelete = deleteBtn.dataset.delete;
      this.#renderList();
      return;
    }
    if (e.target.closest('[data-cancel]')) {
      this.#pendingDelete = null;
      this.#renderList();
      return;
    }
    const confirmBtn = e.target.closest('[data-confirm]');
    if (confirmBtn) await this.#remove(confirmBtn.dataset.confirm);
  }

  async #remove(name) {
    if (this.#busy) return;
    this.#setBusy(true);
    try {
      await this.#scopeAdapter().remove(name);
    } catch (err) {
      this.#showMessage(`Could not delete ${name}: ${err.message}`, 'error');
      this.#setBusy(false);
      return;
    }
    this.#setBusy(false);
    this.#showMessage(`${name} deleted.`, 'ok');
    this.#emitChanged('remove', name);
    await this.refresh();
  }

  /* ── Internals ────────────────────────────────────────────────────────── */

  #scopeAdapter() {
    if (!this.#adapter) this.#adapter = SCOPE_ADAPTERS[this.scope](this.agentId);
    return this.#adapter;
  }

  #savedCopy(name) {
    return this.scope === 'agent'
      ? `${name} saved — restart the agent to apply.`
      : `${name} saved.`;
  }

  #setBusy(busy) {
    this.#busy = busy;
    const submit = this.querySelector('#sm-submit');
    if (submit) submit.disabled = busy;
  }

  #emitChanged(action, name) {
    this.dispatchEvent(new CustomEvent('secrets-changed', {
      bubbles: true,
      detail: { scope: this.scope, action, name },
    }));
  }
}

customElements.define('secrets-manager', SecretsManager);
