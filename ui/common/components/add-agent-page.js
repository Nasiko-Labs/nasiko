import { icons } from '/common/utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (add-agent-page) {
  :scope {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: var(--space-2xl) var(--space-md);
  }
  .page-icon {
    color: var(--color-text-muted);
    margin-bottom: var(--space-md);
  }
  .page-title {
    font-size: var(--font-size-2xl);
    font-weight: 400;
    color: var(--color-text-main);
    margin-bottom: var(--space-xs);
  }
  .page-subtitle {
    font-size: var(--font-size-base);
    color: var(--color-text-muted);
    margin-bottom: var(--space-xl);
  }
  .method-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-lg);
    width: min(100%, 780px);
  }
  @media (max-width: 640px) {
    .method-grid { grid-template-columns: 1fr; }
  }
  .method-card {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-lg);
    padding: var(--space-lg);
    background: var(--color-bg-surface);
    display: flex;
    flex-direction: column;
    gap: var(--space-sm);
    transition: border-color 0.15s;
  }
  .method-card:hover { border-color: var(--color-primary); }
  .method-card.is-disabled {
    opacity: 0.5;
    pointer-events: none;
  }
  .method-card-header {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
  }
  .method-card-title {
    font-size: var(--font-size-lg);
    font-weight: 600;
    color: var(--color-text-main);
  }
  .method-card-req {
    font-size: var(--font-size-xs);
    font-weight: 600;
    color: var(--color-text-muted);
  }
  .method-card-desc {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    line-height: 1.5;
    flex: 1;
  }
  .method-btn {
    margin-top: auto;
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-sm) var(--space-md);
    border: 1px solid var(--color-primary);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--color-primary) 10%, transparent);
    color: var(--color-text-main);
    font-size: var(--font-size-base);
    font-weight: 500;
    cursor: pointer;
    transition: background 0.15s;
  }
  .method-btn:hover {
    background: color-mix(in srgb, var(--color-primary) 20%, transparent);
  }
  .method-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class AddAgentPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <span class="page-icon">${icons.cube('', 48)}</span>
      <h1 class="page-title">Add a new agent</h1>
      <p class="page-subtitle">Choose how you would like to register your agent.</p>

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

        <div class="method-card is-disabled">
          <div class="method-card-header">
            <span>${icons.layers('', 22)}</span>
            <span class="method-card-title">Import from AgentKit</span>
          </div>
          <div class="method-card-req">Coming soon</div>
          <div class="method-card-desc">Sync agents built in AgentKit directly into your registry.</div>
          <button class="method-btn" disabled>Connect AgentKit</button>
        </div>
      </div>
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
        const res = await fetch('/api/catalog/import/upload', { method: 'POST', body: formData });
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
        const res = await fetch('/api/catalog/import/registry', {
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
