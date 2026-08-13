import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import '/common/components/app-button.js';
import '/common/components/app-badge.js';
import '/common/components/app-skeleton.js';
import styles from './add-agent-github-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const GITHUB_ICON = `<svg class="gh-icon" viewBox="0 0 24 24"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>`;

class AddAgentGithubPage extends HTMLElement {
  #allRepos = [];
  #selectedRepo = null;
  #branch = '';
  #agentName = '';
  #githubUsername = null;

  connectedCallback() {
    this.innerHTML = `
      <a class="back-link" href="/add-agent.html">${icons.chevronLeft('', 14)} Back</a>
      <div class="page-head">
        <div>
          <h1 class="title-page">Import from GitHub</h1>
          <p class="subtitle">Connect your GitHub repository, configure options, and register it as an agent</p>
        </div>
      </div>

      <div class="page-layout">
        <div class="steps-col">
          <div class="step-section">
            <p class="step-label">Step 1/3</p>
            <p class="step-title">Select repository</p>
            <p class="step-desc">Choose a repository containing the agent source code</p>
            <div class="search-wrap">
              ${icons.search('', 16)}
              <input type="search" class="search-box" placeholder="Filter repositories..." />
            </div>
            <div class="repo-list">
              <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
              <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
              <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
            </div>
          </div>

          <hr class="step-divider" />

          <div class="step-section step-two">
            <p class="step-label">Step 2/3</p>
            <p class="step-title">Configure options</p>
            <p class="step-desc step-two-desc">Please select a repository in step 1 first</p>
            <div class="config-fields" inert>
              <label class="field">
                <span class="field-label">Branch</span>
                <input type="text" class="field-input branch-input" placeholder="main" />
                <span class="field-hint">Leave blank to use the default branch</span>
              </label>
              <label class="field">
                <span class="field-label">Agent name</span>
                <input type="text" class="field-input name-input" placeholder="Custom agent name" />
                <span class="field-hint">Auto-detected from repository name. Override if needed</span>
              </label>
            </div>
          </div>

          <hr class="step-divider" />

          <div class="step-section step-three">
            <p class="step-label">Step 3/3</p>
            <p class="step-title">Clone and upload</p>
            <p class="step-desc">The selected repository will be cloned and registered as an agent</p>
            <app-button variant="primary" size="sm" class="clone-btn" disabled>Clone and upload</app-button>
          </div>
        </div>

        <div class="status-col">
          <div class="connect-status loading">
            <p class="connect-status-title">Checking GitHub connection...</p>
          </div>
        </div>
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

    this.querySelector('.branch-input').addEventListener('input', (e) => {
      this.#branch = e.target.value;
    });
    this.querySelector('.name-input').addEventListener('input', (e) => {
      this.#agentName = e.target.value;
    });

    this.querySelector('.clone-btn').addEventListener('click', () => this.#doClone());

    this.#checkConnection();
  }

  async #checkConnection() {
    try {
      const tokenRes = await apiFetch('/auth/github/token');
      const tokenBody = await tokenRes.json();

      if (tokenBody.status === 'connected') {
        this.#githubUsername = tokenBody.username || 'GitHub user';
        this.#showConnectedBanner();
        this.#loadRepos();
      } else {
        this.#showDisconnectedBanner();
      }
    } catch {
      this.#showDisconnectedBanner();
    }
  }

  #showConnectedBanner() {
    const el = this.querySelector('.connect-status');
    el.className = 'connect-status connected';
    el.innerHTML = `
      <div class="connect-status-header">
        ${icons.checkSquare?.('', 16) || '&#x2713;'}
        <span class="connect-status-title">Connected to GitHub</span>
      </div>
      <p class="connect-status-sub">Logged in as ${this.#githubUsername}</p>
      <div class="connect-status-actions">
        <app-button size="xs" variant="ghost" class="switch-account-btn">Switch account</app-button>
      </div>
    `;
    el.querySelector('.switch-account-btn')?.addEventListener('click', () => this.#logout());
  }

  #showDisconnectedBanner() {
    const el = this.querySelector('.connect-status');
    el.className = 'connect-status disconnected';
    el.innerHTML = `
      <div class="connect-status-header">
        ${GITHUB_ICON}
        <span class="connect-status-title">Connect to GitHub</span>
      </div>
      <p class="connect-status-sub">Authenticate to access your repositories</p>
      <div class="connect-status-actions">
        <app-button size="sm" variant="primary" class="login-gh-btn">Login with GitHub</app-button>
      </div>
    `;
    el.querySelector('.login-gh-btn').addEventListener('click', () => this.#startGithubAuth());

    // Disable the steps
    const repoList = this.querySelector('.repo-list');
    if (repoList) {
      repoList.innerHTML = '<div class="repo-item" style="justify-content:center;color:var(--color-text-muted);font-size:var(--font-size-sm);">Connect GitHub to view repositories.</div>';
    }
  }

  async #startGithubAuth() {
    const btn = this.querySelector('.login-gh-btn');
    if (btn) { btn.setAttribute('loading', ''); btn.textContent = 'Connecting...'; }

    try {
      const res = await apiFetch('/github/login');
      if (!res.ok) throw new Error((await res.text()) || 'Failed to start GitHub login');
      const { auth_url } = await res.json();
      if (!auth_url) throw new Error('No authorization URL returned');

      const popup = window.open(auth_url, 'github-auth', 'width=600,height=700');
      this.#pollForToken(popup);
    } catch (err) {
      if (btn) { btn.removeAttribute('loading'); btn.textContent = 'Login with GitHub'; }
      const { showToast } = await import('/common/utils/toast.js');
      showToast(`GitHub connect failed: ${err.message}`);
    }
  }

  #pollForToken(popup) {
    let attempts = 0;
    const maxAttempts = 90;

    const timer = setInterval(async () => {
      attempts++;
      if (attempts > maxAttempts) { clearInterval(timer); return; }
      if (popup && popup.closed) {
        clearInterval(timer);
        const btn = this.querySelector('.login-gh-btn');
        if (btn) { btn.removeAttribute('loading'); btn.textContent = 'Login with GitHub'; }
        return;
      }
      try {
        const res = await apiFetch('/auth/github/token');
        if (!res.ok) return;
        const body = await res.json();
        if (body.status === 'connected') {
          clearInterval(timer);
          if (popup && !popup.closed) popup.close();
          this.#githubUsername = body.username || 'GitHub user';
          this.#showConnectedBanner();
          this.#loadRepos();
        }
      } catch { /* keep polling */ }
    }, 2000);
  }

  async #logout() {
    try {
      await apiFetch('/github/logout', { method: 'DELETE' });
    } catch { /* best-effort */ }
    this.#githubUsername = null;
    this.#allRepos = [];
    this.#selectedRepo = null;
    this.#showDisconnectedBanner();
  }

  async #loadRepos() {
    const repoList = this.querySelector('.repo-list');
    if (!repoList) return;
    repoList.innerHTML = `
      <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
      <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
      <div class="repo-item"><app-skeleton height="20px"></app-skeleton></div>
    `;
    try {
      const res = await apiFetch('/github/repositories');
      if (res.status === 403) {
        this.#showDisconnectedBanner();
        return;
      }
      if (res.status === 404) {
        throw new Error('GitHub integration is not configured on this deployment.');
      }
      if (!res.ok) throw new Error((await res.text()) || res.statusText);
      const body = await res.json();
      this.#allRepos = Array.isArray(body) ? body : (body?.repositories || []);
      this.#renderRepos(this.#allRepos);
    } catch (err) {
      repoList.innerHTML = `<p style="color:var(--color-error);padding:var(--space-sm) var(--space-md);">Failed to load repos: ${err.message}</p>`;
    }
  }

  #showConnectGithub() {
    this.innerHTML = '<connect-github redirect="/add-agent-github.html"></connect-github>';
    import('/common/components/connect-github.js');
  }

  #selectRepo(repo) {
    this.#selectedRepo = repo;
    this.#branch = repo.default_branch || 'main';
    this.#agentName = repo.name || repo.full_name.split('/').pop() || '';

    const branchInput = this.querySelector('.branch-input');
    const nameInput = this.querySelector('.name-input');
    branchInput.value = this.#branch;
    nameInput.value = this.#agentName;

    const fields = this.querySelector('.config-fields');
    fields.removeAttribute('inert');

    const desc = this.querySelector('.step-two-desc');
    desc.textContent = 'Configure repository options';

    this.querySelector('.clone-btn').removeAttribute('disabled');

    this.querySelectorAll('.repo-radio').forEach(r => {
      r.dataset.selected = r.dataset.repo === repo.full_name ? 'true' : 'false';
    });
  }

  async #doClone() {
    if (!this.#selectedRepo) return;
    const btn = this.querySelector('.clone-btn');
    btn.setAttribute('loading', '');
    btn.textContent = 'Cloning and uploading...';

    const payload = { repository_full_name: this.#selectedRepo.full_name };
    if (this.#branch) payload.branch = this.#branch;
    if (this.#agentName) payload.agent_name = this.#agentName;

    try {
      const res = await apiFetch('/github/clone', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
      if (!res.ok) throw new Error(await res.text());
      window.location.href = '/your-agents.html';
    } catch (err) {
      btn.removeAttribute('loading');
      btn.textContent = 'Clone and upload';
      const { showToast } = await import('/common/utils/toast.js');
      showToast(`Clone failed: ${err.message}`);
    }
  }

  #renderRepos(repos) {
    const repoList = this.querySelector('.repo-list');
    if (!repos.length) {
      repoList.innerHTML = '<div class="repo-item" style="justify-content:center;color:var(--color-text-muted);font-size:var(--font-size-sm);">No repositories found.</div>';
      return;
    }
    repoList.innerHTML = repos.map(r => `
      <div class="repo-item repo-selectable" data-full-name="${r.full_name}">
        <span class="repo-radio" data-repo="${r.full_name}" data-selected="${this.#selectedRepo?.full_name === r.full_name}"></span>
        <div class="repo-info">
          <span class="repo-name">${r.full_name}</span>
          <span class="repo-meta">
            ${r.language ? `<span>${r.language}</span>` : ''}
            ${r.updated_at ? `<span>${new Date(r.updated_at).toLocaleDateString()}</span>` : ''}
            ${r.private ? '<app-badge size="xs" variant="neutral">Private</app-badge>' : ''}
          </span>
        </div>
      </div>
    `).join('');

    repoList.querySelectorAll('.repo-selectable').forEach(row => {
      row.addEventListener('click', () => {
        const fullName = row.dataset.fullName;
        const repo = this.#allRepos.find(r => r.full_name === fullName);
        if (repo) this.#selectRepo(repo);
      });
    });
  }
}

customElements.define('add-agent-github-page', AddAgentGithubPage);