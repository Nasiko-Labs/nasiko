const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (skills-page) {
  :scope {
    display: block;
    max-width: 1200px;
    margin: 0 auto;
    padding: var(--space-xl);
  }
  .title {
    font-size: var(--font-size-2xl);
    font-weight: 500;
    margin-bottom: var(--space-xs);
  }
  .subtitle {
    color: var(--color-text-muted);
    font-size: var(--font-size-sm);
    margin-bottom: var(--space-lg);
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const STATUS_VARIANTS = { preview: 'warning', stable: 'success', verified: 'info', yanked: 'error' };

class SkillsPage extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <h1 class="title">Skills</h1>
      <p class="subtitle">Reusable tools that can be added to any agent project.</p>
      <smart-table
        id="skills-table"
        data-fn="fetchSkillArtifacts"
        search
        search-placeholder="Search skills..."
        limit="20"
      ></smart-table>
    `;

    const table = this.querySelector('#skills-table');
    table.columns = [
      { key: 'name', label: 'Skill', width: '22%', render: (v) => `<span style="font-weight:500;">${v}</span>` },
      { key: 'framework', label: 'Framework', width: '12%', render: (v) => v ? `<code style="font-size:var(--font-size-xs);background:var(--color-bg-base);padding:2px 6px;border-radius:var(--radius-sm);">${v}</code>` : '—' },
      { key: 'version', label: 'Version', width: '10%', render: (v) => v || '—' },
      { key: 'status', label: 'Status', width: '10%', render: (v) => `<app-badge variant="${STATUS_VARIANTS[v] || 'neutral'}">${v}</app-badge>` },
      { key: 'description', label: 'Description', width: '30%', render: (v) => `<span style="color:var(--color-text-muted);font-size:var(--font-size-sm);">${v || '—'}</span>` },
      { key: 'tags', label: 'Tags', width: '16%', render: (v) => (v || []).slice(0, 4).map(t => `<app-badge variant="neutral">${t}</app-badge>`).join(' ') || '—' },
    ];
  }
}

customElements.define('skills-page', SkillsPage);
