import { icons } from '/common/utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (add-agent-github-page) {
  :scope { display: block; }
  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-xs);
  }
  .toolbar h1 { font-size: var(--font-size-lg); font-weight: 600; margin: 0; }
  .back-link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--font-size-sm);
    color: var(--color-primary);
    text-decoration: none;
  }
  .back-link:hover { text-decoration: underline; }
  .subtitle {
    color: var(--color-text-muted);
    font-size: var(--font-size-xs);
    margin-bottom: var(--space-md);
  }
  .subtitle code {
    padding: 1px 5px;
    background: var(--color-bg-elevated);
    border-radius: var(--radius-sm);
    font-size: var(--font-size-xs);
  }
  .repo-list {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    overflow: hidden;
    margin-top: var(--space-sm);
  }
  .repo-item {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    padding: var(--space-sm) var(--space-md);
    border-bottom: 1px solid var(--color-border);
    background: var(--color-bg-surface);
    transition: background 0.1s;
  }
  .repo-item:last-child { border-bottom: none; }
  .repo-item:hover { background: var(--color-bg-elevated); }
  .repo-info { flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-md); }
  .repo-name {
    font-weight: 500;
    font-size: var(--font-size-sm);
    color: var(--color-text-main);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    font-family: var(--font-mono);
  }
  .repo-meta {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    white-space: nowrap;
  }
  .search-box {
    width: 100%;
    padding: var(--space-xs) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    font-size: var(--font-size-sm);
    color: var(--color-text-main);
  }
  .search-box:focus {
    outline: none;
    border-color: var(--color-primary);
    box-shadow: 0 0 0 2px var(--color-primary-ring);
  }
  .search-box::placeholder { color: var(--color-text-muted); }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class AddAgentGithubPage extends HTMLElement {
  #allRepos = [];

  connectedCallback() {
    this.innerHTML = `
      <div class="toolbar">
        <h1>Import from GitHub</h1>
        <a class="back-link" href="/add-agent.html">${icons.chevronLeft('', 14)} Back</a>
      </div>
      <p class="subtitle">Select a repository containing a Nasiko agent. The repo must include an <code>AgentCard.json</code> at the root.</p>
      <input type="search" class="search-box" placeholder="Filter repositories..." />
      <div class="repo-list">
        <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
        <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
        <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
      </div>
    `;

    this.querySelector('.search-box').addEventListener('input', (e) => {
      const q = e.target.value.toLowerCase();
      const filtered = this.#allRepos.filter(r =>
        r.full_name.toLowerCase().includes(q) ||
        (r.description || '').toLowerCase().includes(q)
      );
      this.#renderRepos(filtered);
    });

    this.addEventListener('click', async (e) => {
      const btn = e.target.closest('[data-import]');
      if (!btn) return;
      const repo = btn.dataset.import;
      btn.setAttribute('loading', '');
      try {
        const res = await fetch('/api/catalog/import/github', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ repository: repo }),
        });
        if (!res.ok) throw new Error(await res.text());
        window.location.href = '/your-agents.html';
      } catch (err) {
        btn.removeAttribute('loading');
        const { showToast } = await import('/common/utils/toast.js');
        showToast(`Import failed: ${err.message}`);
      }
    });

    this.#loadRepos();
  }

  async #loadRepos() {
    const repoList = this.querySelector('.repo-list');
    try {
      const res = await fetch('/api/auth/github/repos');
      if (res.status === 401 || res.status === 403) {
        this.#showConnectGithub();
        return;
      }
      if (!res.ok) throw new Error(res.statusText);
      this.#allRepos = await res.json();
      this.#renderRepos(this.#allRepos);
    } catch (err) {
      repoList.innerHTML = `<p style="color:var(--color-error);">Failed to load repos: ${err.message}</p>`;
    }
  }

  #showConnectGithub() {
    this.innerHTML = '<connect-github redirect="/add-agent-github.html"></connect-github>';
    import('/common/components/connect-github.js');
  }

  #renderRepos(repos) {
    const repoList = this.querySelector('.repo-list');
    if (!repos.length) {
      repoList.innerHTML = '<div class="repo-item" style="justify-content:center;color:var(--color-text-muted);font-size:var(--font-size-sm);">No repositories found.</div>';
      return;
    }
    repoList.innerHTML = repos.map(r => `
      <div class="repo-item">
        <div class="repo-info">
          <span class="repo-name">${r.full_name}</span>
          <span class="repo-meta">
            ${r.language ? `<span>${r.language}</span>` : ''}
            ${r.updated_at ? `<span>${new Date(r.updated_at).toLocaleDateString()}</span>` : ''}
            ${r.private ? '<app-badge size="xs" variant="neutral">Private</app-badge>' : ''}
          </span>
        </div>
        <app-button size="xs" data-import="${r.full_name}">Import</app-button>
      </div>
    `).join('');
  }
}

customElements.define('add-agent-github-page', AddAgentGithubPage);
