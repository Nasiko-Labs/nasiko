import { apiFetch } from '/common/services/api.js';
import styles from './connect-github.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const GITHUB_ICON = `<svg viewBox="0 0 24 24"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>`;

class ConnectGithub extends HTMLElement {
  connectedCallback() {
    this.innerHTML = `
      <div class="connect-card">
        <div class="connect-icon">${GITHUB_ICON}</div>
        <h2 class="connect-title">Connect GitHub</h2>
        <p class="connect-desc">
          Connect your GitHub account to browse repositories, deploy agents directly from source, and enable automated builds.
        </p>
        <button class="connect-btn" type="button">
          ${GITHUB_ICON}
          Connect GitHub Account
        </button>
        <p class="connect-scopes">Permissions requested: repository access, user profile</p>
      </div>
    `;

    this.querySelector('.connect-btn').addEventListener('click', () => this.#startAuth());
  }

  async #startAuth() {
    const btn = this.querySelector('.connect-btn');
    btn.disabled = true;
    btn.textContent = 'Connecting...';

    try {
      const res = await apiFetch('/github/login');
      if (!res.ok) throw new Error((await res.text()) || 'Failed to start GitHub login');
      const { auth_url } = await res.json();
      if (!auth_url) throw new Error('No authorization URL returned');

      // Open GitHub OAuth in a popup and poll for completion.
      const popup = window.open(auth_url, 'github-auth', 'width=600,height=700');
      this.#pollForToken(popup);
    } catch (err) {
      btn.disabled = false;
      btn.innerHTML = `${GITHUB_ICON} Connect GitHub Account`;
      const { showToast } = await import('/common/utils/toast.js');
      showToast(`GitHub connect failed: ${err.message}`);
    }
  }

  #pollForToken(popup) {
    let attempts = 0;
    const maxAttempts = 90; // 3 minutes at 2s intervals
    const redirectPath = this.getAttribute('redirect') || window.location.pathname;

    const timer = setInterval(async () => {
      attempts++;
      if (attempts > maxAttempts) {
        clearInterval(timer);
        return;
      }
      // If the user closed the popup, stop polling.
      if (popup && popup.closed) {
        clearInterval(timer);
        const btn = this.querySelector('.connect-btn');
        if (btn) {
          btn.disabled = false;
          btn.innerHTML = `${GITHUB_ICON} Connect GitHub Account`;
        }
        return;
      }
      try {
        const res = await apiFetch('/auth/github/token');
        if (!res.ok) return; // keep polling
        const body = await res.json();
        if (body.connected || body.status === 'connected') {
          clearInterval(timer);
          if (popup && !popup.closed) popup.close();
          window.location.href = redirectPath;
        }
      } catch { /* keep polling */ }
    }, 2000);
  }
}

customElements.define('connect-github', ConnectGithub);
