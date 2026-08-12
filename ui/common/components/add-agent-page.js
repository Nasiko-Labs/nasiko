import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import '/common/components/app-modal.js';
import styles from './add-agent-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

/// Mirrors the server's `validate_version_tag` (oss/server/src/build/routes.rs),
/// which every uploaded agent name must satisfy because it becomes part of an
/// OCI image reference.
const AGENT_NAME_RE = /^[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}$/;

/// Best-effort coercion of arbitrary text into a valid agent name. Returns ''
/// when nothing usable survives, so the caller still asks the user.
function sanitizeAgentName(raw) {
  const cleaned = raw
    .trim()
    .toLowerCase()
    .replace(/[^a-zA-Z0-9._-]+/g, '-')  // spaces, parens, slashes → separator
    .replace(/^[^a-zA-Z0-9_]+/, '')     // leading . or - is rejected by the server
    .replace(/-{2,}/g, '-')
    .replace(/-+$/, '')
    .slice(0, 128);
  return AGENT_NAME_RE.test(cleaned) ? cleaned : '';
}

/// Null when `name` is acceptable, otherwise the reason to show the user.
function agentNameError(name) {
  if (!name) return 'An agent name is required.';
  if (AGENT_NAME_RE.test(name)) return null;
  if (name.length > 128) return 'Agent name must be 128 characters or fewer.';
  if (!/^[a-zA-Z0-9_]/.test(name)) return 'Agent name must start with a letter, digit or underscore.';
  return 'Agent name may only contain letters, digits, dots, underscores and hyphens.';
}

class AddAgentPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <span class="page-icon">${icons.cube('', 28)}</span>
      <h1 class="title-page">Import new agent</h1>
      <p class="page-subtitle">Choose how you would like to register your agent.</p>

      <a class="cli-banner" href="/setup-cli.html">
        <span class="cli-banner-icon">${icons.terminal('', 18)}</span>
        <span class="cli-banner-text">
          <span class="cli-banner-title">Set up CLI</span>
          <span class="cli-banner-sub">Prefer the terminal? Build, test, and publish agents with the Nasiko CLI.</span>
        </span>
        <span class="cli-banner-chevron">${icons.chevronRight('', 16)}</span>
      </a>

      <div class="method-grid">
        <div class="method-card">
          <div class="method-card-header">
            <span>${icons.github('', 22)}</span>
            <span class="method-card-title">Import from GitHub</span>
          </div>
          <div class="method-card-req">Requires GitHub authentication</div>
          <div class="method-card-desc">Pull your agent from a GitHub repository. Keep code and metadata in sync.</div>
          <button class="method-btn" id="btn-github">Connect GitHub</button>
        </div>

        <div class="method-card">
          <div class="method-card-header">
            <span>${icons.upload('', 22)}</span>
            <span class="method-card-title">Upload a code package</span>
          </div>
          <div class="method-card-req">Include skill.json in the package</div>
          <div class="method-card-desc">Register your agent with a .zip that includes source and config.</div>
          <button class="method-btn" id="btn-upload">Upload .zip</button>
        </div>

        <div class="method-card">
          <div class="method-card-header">
            <span>${icons.layers('', 22)}</span>
            <span class="method-card-title">Import from OCI registry</span>
          </div>
          <div class="method-card-req">You'll need the image URL</div>
          <div class="method-card-desc">Pull a pre-built agent image directly from any OCI-compatible container registry.</div>
          <button class="method-btn" id="btn-oci">Connect Registry</button>
        </div>
      </div>

      <app-modal id="upload-modal" heading="Upload agent package">
        <div class="upload-form">
          <label class="field">
            <span class="field-label">Source archive (.zip)</span>
            <input type="file" id="upload-file" accept=".zip,application/zip" required />
          </label>
          <label class="field">
            <span class="field-label">Agent name</span>
            <input type="text" id="upload-name" autocomplete="off" placeholder="my-agent" />
            <span class="field-hint">Letters, digits, dots, underscores and hyphens; must start with
              a letter, digit or underscore. Pre-filled from the file name.</span>
          </label>
          <p class="form-error" id="upload-error" hidden></p>
        </div>
        <div data-slot="footer">
          <button type="button" class="btn-outline" id="upload-cancel">Cancel</button>
          <button type="button" class="btn-dark" id="upload-submit">Upload and deploy</button>
        </div>
      </app-modal>
    `;

    this.querySelector('#btn-github')?.addEventListener('click', () => {
      window.location.href = '/add-agent-github.html';
    });

    this.#wireUploadModal();

    this.querySelector('#btn-oci')?.addEventListener('click', async () => {
      const reference = prompt('Enter artifact reference (e.g. nasiko/my-agent:v1.0):');
      if (!reference) return;
      try {
        const res = await apiFetch('/import/registry', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ reference }),
        });
        if (!res.ok) throw new Error(await res.text());
        window.location.href = '/your-agents.html';
      } catch (err) {
        const { showToast } = await import('/common/utils/toast.js');
        showToast(`Import failed: ${err.message}`);
      }
    });
  }

  /// The agent name becomes part of an OCI image reference, so the server
  /// enforces `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}` on it (`validate_version_tag`).
  /// A raw file name routinely violates that ("My Agent.zip", "agent (1).zip"),
  /// which used to surface as an unexplained 400 with no way to correct it —
  /// hence a real form: sanitized suggestion, editable, validated before send.
  #wireUploadModal() {
    const modal = this.querySelector('#upload-modal');
    const fileEl = this.querySelector('#upload-file');
    const nameEl = this.querySelector('#upload-name');
    const errorEl = this.querySelector('#upload-error');
    const submitEl = this.querySelector('#upload-submit');

    this.querySelector('#btn-upload')?.addEventListener('click', () => {
      fileEl.value = '';
      nameEl.value = '';
      errorEl.hidden = true;
      modal.open();
    });

    this.querySelector('#upload-cancel').addEventListener('click', () => modal.close());

    // Suggest a name from the chosen file, but never overwrite one the user
    // has already typed.
    fileEl.addEventListener('change', () => {
      const file = fileEl.files[0];
      if (!file || nameEl.value.trim()) return;
      nameEl.value = sanitizeAgentName(file.name.replace(/\.zip$/i, ''));
    });

    submitEl.addEventListener('click', async () => {
      const file = fileEl.files[0];
      const name = nameEl.value.trim();
      errorEl.hidden = true;

      if (!file) {
        this.#showUploadError('Choose a .zip archive to upload.');
        return;
      }
      const invalid = agentNameError(name);
      if (invalid) {
        this.#showUploadError(invalid);
        return;
      }

      const formData = new FormData();
      formData.append('name', name);
      formData.append('file', file);

      submitEl.disabled = true;
      try {
        const res = await apiFetch('/agents/upload', { method: 'POST', body: formData });
        if (!res.ok) throw new Error((await res.text()) || `HTTP ${res.status}`);
        window.location.href = '/your-agents.html';
      } catch (err) {
        this.#showUploadError(`Upload failed: ${err.message}`);
      } finally {
        submitEl.disabled = false;
      }
    });
  }

  #showUploadError(message) {
    const el = this.querySelector('#upload-error');
    el.textContent = message;
    el.hidden = false;
  }
}

customElements.define('add-agent-page', AddAgentPage);
