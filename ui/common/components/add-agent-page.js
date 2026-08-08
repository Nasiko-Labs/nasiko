import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import styles from './add-agent-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

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

      <p class="coming-soon">Coming soon... Import from AgentKit.</p>
    `;

    const fileInput = document.createElement('input');
    fileInput.type = 'file';
    fileInput.accept = '.zip';

    this.querySelector('#btn-github')?.addEventListener('click', () => {
      window.location.href = '/add-agent-github.html';
    });

    this.querySelector('#btn-upload')?.addEventListener('click', () => fileInput.click());

    fileInput.addEventListener('change', async () => {
      const file = fileInput.files[0];
      if (!file) return;
      const formData = new FormData();
      formData.append('package', file);
      try {
        const res = await apiFetch('/import/upload', { method: 'POST', body: formData });
        if (!res.ok) throw new Error(await res.text());
        window.location.href = '/your-agents.html';
      } catch (err) {
        const { showToast } = await import('/common/utils/toast.js');
        showToast(`Upload failed: ${err.message}`);
      }
    });

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
}

customElements.define('add-agent-page', AddAgentPage);
