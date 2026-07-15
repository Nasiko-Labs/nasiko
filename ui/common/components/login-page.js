import { icons } from '../utils/icons.js';
import styles from './login-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const GITHUB_ICON = `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>`;
const LOGO_ICON = `<svg viewBox="0 0 24 24"><path d="M3 3h18v18H3V3zm2 2v14h14V5H5zm3 3h2v8H8V8zm3 2h2v6h-2v-6zm3-1h2v7h-2V9z"/></svg>`;

class LoginPage extends HTMLElement {
  async connectedCallback() {
    const brandTitle = this.getAttribute('brand-title') || 'Nasiko';
    const subtitle = this.getAttribute('subtitle') || 'Sign in to your workspace';
    const showGithub = !this.hasAttribute('no-github');
    const showGoogle = !this.hasAttribute('no-google');
    const showCredentials = !this.hasAttribute('no-credentials');

    // Microsoft/OIDC is opt-in per deployment (unlike GitHub/Google, which
    // are always offered) — only show the button once the backend confirms
    // OIDC_ISSUER_URL/CLIENT_ID/CLIENT_SECRET/REDIRECT_URI (or the
    // DB-configured equivalent, see `resolve_oidc_client`) are actually set,
    // so a deployment that hasn't configured SSO never shows a button that
    // would just 503. Fails closed (hidden) on a network error.
    let showMicrosoft = false;
    if (!this.hasAttribute('no-microsoft')) {
      try {
        const res = await fetch('/api/auth/oidc/status', { credentials: 'same-origin' });
        const data = await res.json();
        showMicrosoft = Boolean(data?.configured);
      } catch {
        showMicrosoft = false;
      }
    }

    let oauthSection = '';
    if (showGithub || showGoogle || showMicrosoft) {
      let buttons = '';
      if (showMicrosoft) buttons += `<a href="/api/auth/oidc/login" class="btn-oauth">${icons.microsoft} Continue with Microsoft</a>`;
      if (showGithub) buttons += `<a href="/api/auth/github" class="btn-oauth">${GITHUB_ICON} Continue with GitHub</a>`;
      if (showGoogle) buttons += `<a href="/api/auth/google" class="btn-oauth">${icons.google} Continue with Google</a>`;
      oauthSection = `
        <div class="divider">or</div>
        <div class="oauth-section">${buttons}</div>
      `;
    }

    this.innerHTML = `
      <div class="card">
        <div class="brand">
          <div class="brand-icon">${LOGO_ICON}</div>
          <span class="brand-text">${brandTitle}</span>
        </div>
        <p class="subtitle">${subtitle}</p>
        ${showCredentials ? `
          <form id="login-form">
            <div class="field">
              <label for="username">Username</label>
              <input type="text" id="username" placeholder="admin" autocomplete="username" required />
            </div>
            <div class="field">
              <label for="password">Password</label>
              <input type="password" id="password" placeholder="password" autocomplete="current-password" required />
            </div>
            <div class="error-msg" id="error-msg"></div>
            <button type="submit" class="btn-submit" id="submit-btn">Sign In</button>
          </form>
        ` : ''}
        ${oauthSection}
      </div>
    `;

    if (showCredentials) this.#setupForm();
  }

  #setupForm() {
    const form = this.querySelector('#login-form');
    const errorMsg = this.querySelector('#error-msg');
    const submitBtn = this.querySelector('#submit-btn');

    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      errorMsg.classList.remove('visible');
      submitBtn.disabled = true;
      submitBtn.textContent = 'Signing in…';

      try {
        const res = await fetch('/api/auth/login', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'same-origin',
          body: JSON.stringify({
            username: this.querySelector('#username').value,
            password: this.querySelector('#password').value,
          }),
        });

        if (!res.ok) {
          const data = await res.json().catch(() => null);
          throw new Error(data?.error || 'Invalid credentials');
        }

        window.location.href = '/';
      } catch (err) {
        errorMsg.textContent = err.message;
        errorMsg.classList.add('visible');
      } finally {
        submitBtn.disabled = false;
        submitBtn.textContent = 'Sign In';
      }
    });
  }
}

customElements.define('login-page', LoginPage);
