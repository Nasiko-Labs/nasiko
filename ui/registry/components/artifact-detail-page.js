import { icons } from '/common/utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (artifact-detail-page) {
  :scope {
    display: block;
    max-width: 800px;
    margin: 0 auto;
    padding: var(--space-2xl) var(--space-md);
  }
  .back {
    display: inline-flex;
    align-items: center;
    gap: var(--space-xs);
    color: var(--color-text-muted);
    text-decoration: none;
    font-size: var(--font-size-sm);
    margin-bottom: var(--space-lg);
  }
  .back:hover { color: var(--color-primary); }
  .header { margin-bottom: var(--space-xl); }
  .header h1 {
    font-size: var(--font-size-2xl);
    font-weight: 500;
    margin-bottom: var(--space-xs);
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    flex-wrap: wrap;
  }
  .desc { color: var(--color-text-muted); font-size: var(--font-size-sm); }
  .meta-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: var(--space-md);
    margin-bottom: var(--space-xl);
  }
  .meta-card {
    background: var(--color-bg-surface);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
  }
  .meta-card .label {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: var(--space-xs);
  }
  .meta-card .value { font-size: var(--font-size-base); font-weight: 500; }
  .tags { display: flex; flex-wrap: wrap; gap: var(--space-xs); margin-bottom: var(--space-xl); }
  .section { margin-top: var(--space-xl); }
  .section h2 { font-size: var(--font-size-lg); font-weight: 500; margin-bottom: var(--space-md); }
  .install-cmd {
    background: var(--color-bg-base);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    font-family: var(--font-mono);
    font-size: var(--font-size-sm);
    overflow-x: auto;
    user-select: all;
  }
  .oci-url-row {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
  }
  .oci-url-row .install-cmd { flex: 1; }
  .btn-copy {
    padding: var(--space-xs) var(--space-sm);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: var(--space-xs);
    white-space: nowrap;
  }
  .btn-copy:hover { border-color: var(--color-primary); }
  .actions { margin-top: var(--space-xl); display: flex; gap: var(--space-sm); }
  .btn-danger {
    padding: var(--space-xs) var(--space-md);
    background: var(--color-error-bg);
    color: var(--color-error);
    border: 1px solid var(--color-error-border);
    border-radius: var(--radius-md);
    font-size: var(--font-size-sm);
    cursor: pointer;
  }
  .btn-danger:hover { background: var(--color-error); color: white; }
  .loading { color: var(--color-text-muted); font-style: italic; }
  .error { color: var(--color-error); }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { preview: 'warning', stable: 'success', verified: 'info', yanked: 'error' };
const TYPE_VARIANTS = { agent: 'info', skill: 'success', tool: 'neutral' };

class ArtifactDetailPage extends HTMLElement {
  connectedCallback() {
    this.#load();
  }

  async #load() {
    const params = new URLSearchParams(window.location.search);
    const owner = params.get('owner');
    const name = params.get('name');

    if (!owner || !name) {
      this.innerHTML = `<p class="error">Missing owner or name parameter.</p>`;
      return;
    }

    this.innerHTML = `<p class="loading">Loading...</p>`;

    try {
      const res = await fetch(`/v1/artifacts/${owner}/${name}`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const { data } = await res.json();
      this.#render(data);
    } catch (err) {
      this.innerHTML = `<p class="error">Failed to load artifact: ${err.message}</p>`;
    }
  }

  #render(artifact) {
    const tags = (artifact.tags || []).map(t => `<app-badge variant="neutral">${t}</app-badge>`).join('');
    const installCmd = artifact.artifact_type === 'skill'
      ? `nasiko skill add ${artifact.name}`
      : `nasiko new ${artifact.owner}/${artifact.name} my-agent`;

    this.innerHTML = `
      <a class="back" href="/index.html">${icons.chevronLeft('', 16)} All artifacts</a>

      <div class="header">
        <h1>
          ${artifact.owner}/${artifact.name}
          <app-badge variant="${TYPE_VARIANTS[artifact.artifact_type] || 'neutral'}">${artifact.artifact_type}</app-badge>
          <app-badge variant="${STATUS_VARIANTS[artifact.status] || 'neutral'}">${artifact.status}</app-badge>
        </h1>
        <p class="desc">${artifact.description || 'No description.'}</p>
      </div>

      <div class="meta-grid">
        <div class="meta-card">
          <div class="label">Version</div>
          <div class="value">${artifact.version}</div>
        </div>
        <div class="meta-card">
          <div class="label">Framework</div>
          <div class="value">${artifact.framework || '—'}</div>
        </div>
        <div class="meta-card">
          <div class="label">License</div>
          <div class="value">${artifact.license || '—'}</div>
        </div>
        <div class="meta-card">
          <div class="label">OCI Digest</div>
          <div class="value" style="font-family:var(--font-mono);font-size:var(--font-size-xs);">${artifact.oci_digest ? artifact.oci_digest.slice(0, 20) + '...' : 'not pushed'}</div>
        </div>
      </div>

      ${tags ? `<div class="tags">${tags}</div>` : ''}

      ${artifact.artifact_type === 'agent' && artifact.oci_digest ? `
      <div class="section">
        <h2>Deploy to Control Plane</h2>
        <p class="desc" style="margin-bottom:var(--space-sm);">Use this OCI image URL in your control plane's "Import from OCI registry" to deploy this agent.</p>
        <div class="oci-url-row">
          <div class="install-cmd" id="oci-url">${window.location.host}/${artifact.owner}/${artifact.name}:${artifact.version}</div>
          <button class="btn-copy" id="btn-copy-oci">${icons.copy('', 16)} Copy</button>
        </div>
      </div>` : ''}

      <div class="section">
        <h2>Install via CLI</h2>
        <div class="install-cmd">${installCmd}</div>
      </div>

      <div class="actions">
        <button class="btn-danger" id="yank-btn">Yank ${artifact.version}</button>
      </div>
    `;

    this.querySelector('#yank-btn')?.addEventListener('click', () => this.#yank(artifact));
    this.querySelector('#btn-copy-oci')?.addEventListener('click', () => {
      const url = this.querySelector('#oci-url')?.textContent;
      if (url) {
        navigator.clipboard.writeText(url);
        const btn = this.querySelector('#btn-copy-oci');
        btn.textContent = 'Copied!';
        setTimeout(() => { btn.innerHTML = `${icons.copy('', 16)} Copy`; }, 2000);
      }
    });
  }

  async #yank(artifact) {
    if (!confirm(`Yank ${artifact.owner}/${artifact.name}:${artifact.version}?`)) return;
    try {
      await window.yankArtifact(artifact.owner, artifact.name, artifact.version);
      this.#load();
    } catch (err) {
      alert(`Yank failed: ${err.message}`);
    }
  }
}

customElements.define('artifact-detail-page', ArtifactDetailPage);
