import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import styles from './add-agent-github-page.css' with { type: 'css' };
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
        const res = await apiFetch('/import/github', {
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
      const res = await apiFetch('/auth/github/repos');
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
