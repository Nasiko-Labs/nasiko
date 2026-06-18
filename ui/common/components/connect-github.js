const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (connect-github) {
  :scope { display: block; }
  .connect-card {
    max-width: 480px;
    margin: var(--space-xl) auto;
    padding: var(--space-2xl);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-lg);
    background: var(--color-bg-surface);
    text-align: center;
  }
  .connect-icon {
    width: 64px;
    height: 64px;
    margin: 0 auto var(--space-lg);
    border-radius: 50%;
    background: var(--color-neutral-bg);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .connect-icon svg { width: 32px; height: 32px; fill: var(--color-text-main); }
  .connect-title {
    font-size: var(--font-size-xl);
    font-weight: 600;
    color: var(--color-text-main);
    margin-bottom: var(--space-sm);
  }
  .connect-desc {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    margin-bottom: var(--space-xl);
    line-height: 1.6;
  }
  .connect-btn {
    display: inline-flex;
    align-items: center;
    gap: var(--space-sm);
    padding: 12px var(--space-xl);
    background: #24292f;
    color: #fff;
    font-weight: 600;
    font-size: var(--font-size-base);
    border-radius: var(--radius-md);
    border: none;
    cursor: pointer;
    text-decoration: none;
    transition: background 0.15s;
  }
  .connect-btn:hover { background: #1b1f23; color: #fff; }
  .connect-btn svg { width: 20px; height: 20px; fill: currentColor; }
  .connect-scopes {
    margin-top: var(--space-lg);
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const GITHUB_ICON = `<svg viewBox="0 0 24 24"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>`;

class ConnectGithub extends HTMLElement {
  connectedCallback() {
    const redirectPath = this.getAttribute('redirect') || window.location.pathname + window.location.search;
    const scope = this.getAttribute('scope') || 'repo,read:user';

    this.innerHTML = `
      <div class="connect-card">
        <div class="connect-icon">${GITHUB_ICON}</div>
        <h2 class="connect-title">Connect GitHub</h2>
        <p class="connect-desc">
          Connect your GitHub account to browse repositories, deploy agents directly from source, and enable automated builds.
        </p>
        <a href="/api/auth/github/connect?scope=${encodeURIComponent(scope)}&redirect=${encodeURIComponent(redirectPath)}" class="connect-btn">
          ${GITHUB_ICON}
          Connect GitHub Account
        </a>
        <p class="connect-scopes">Permissions requested: repository access, user profile</p>
      </div>
    `;
  }
}

customElements.define('connect-github', ConnectGithub);
