const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (artifacts-page) {
  :scope {
    display: block;
    max-width: 1200px;
    margin: 0 auto;
    padding: var(--space-xl);
  }
  .title {
    font-size: var(--font-size-2xl);
    font-weight: 500;
    margin-bottom: var(--space-lg);
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const TYPE_VARIANTS = { agent: 'info', skill: 'success', tool: 'neutral' };
const STATUS_VARIANTS = { preview: 'warning', stable: 'success', verified: 'info', yanked: 'error' };

class ArtifactsPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <h1 class="title">Artifacts</h1>
      <smart-table
        id="artifacts-table"
        data-fn="fetchArtifacts"
        search
        search-placeholder="Search agents, skills, and tools..."
        limit="20"
      ></smart-table>
    `;

    const table = this.querySelector('#artifacts-table');
    table.columns = [
      { key: 'name', label: 'Name', width: '22%', render: (v, row) => `<a href="/artifact.html?owner=${row.owner}&name=${v}" style="color:var(--color-primary);text-decoration:none;font-weight:500;">${row.owner}/${v}</a>` },
      { key: 'artifact_type', label: 'Type', width: '10%', render: (v) => `<app-badge variant="${TYPE_VARIANTS[v] || 'neutral'}">${v}</app-badge>` },
      { key: 'version', label: 'Version', width: '10%', render: (v) => `<code style="font-size:var(--font-size-xs);background:var(--color-bg-base);padding:2px 6px;border-radius:var(--radius-sm);">${v}</code>` },
      { key: 'framework', label: 'Framework', width: '12%', render: (v) => v || '—' },
      { key: 'status', label: 'Status', width: '10%', render: (v) => `<app-badge variant="${STATUS_VARIANTS[v] || 'neutral'}">${v}</app-badge>` },
      { key: 'description', label: 'Description', width: '26%', render: (v) => `<span style="color:var(--color-text-muted);font-size:var(--font-size-sm);">${v || '—'}</span>` },
      { key: 'tags', label: 'Tags', width: '10%', render: (v) => (v || []).slice(0, 3).map(t => `<app-badge variant="neutral">${t}</app-badge>`).join(' ') || '—' },
    ];
  }
}

customElements.define('artifacts-page', ArtifactsPage);
