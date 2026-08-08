import { icons } from '../utils/icons.js';
import '../utils/theme.js'; // side effect: applies the pinned theme (no app-header here)
import styles from './login-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const GITHUB_ICON = `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>`;
// Nasiko brand mark (barcode "N") — inlined copy of /common/mark-nasiko.svg
// with the bars recolored to currentColor so it adapts to the login card.
const LOGO_ICON = `<svg viewBox="0 0 64 64" fill="none"><g fill="currentColor"><rect width="3.29" height="53.74" rx="1.64"/><rect x="5.52" width="3.29" height="58.45" rx="1.64"/><rect x="11.04" width="3.29" height="63.82" rx="1.64"/><rect x="16.56" width="3.29" height="63.82" rx="1.64"/><rect x="22.08" width="3.29" height="22.84" rx="1.64"/><rect x="27.6" width="3.29" height="22.84" rx="1.64"/><rect x="33.12" width="3.29" height="27.54" rx="1.64"/><rect x="38.63" width="3.29" height="32.25" rx="1.64"/><rect x="44.15" width="3.29" height="22.84" rx="1.64"/><rect x="49.56" y="6.03" width="3.35" height="16.74" rx="1.67"/><rect x="55.19" y="10.26" width="3.29" height="53.74" rx="1.64"/><rect x="60.71" y="14.82" width="3.29" height="49.04" rx="1.64"/><rect x="22.31" y="53.56" width="3.35" height="10.26" rx="1.67"/><rect x="27.89" y="53.56" width="3.29" height="10.08" rx="1.64"/><rect x="33.47" y="43.31" width="3.35" height="20.51" rx="1.67"/><rect x="39.05" y="47.87" width="3.35" height="15.96" rx="1.67"/><rect x="44.39" y="53.56" width="3.35" height="10.26" rx="1.67"/><rect x="49.97" y="53.56" width="3.35" height="10.26" rx="1.67"/></g></svg>`;

class LoginPage extends HTMLElement {
  async connectedCallback() {
    const brandTitle = this.getAttribute('brand-title') || 'Nasiko';
    const subtitle = this.getAttribute('subtitle') || 'Sign in to your workspace';
    const showGithub = !this.hasAttribute('no-github');
    let showGoogle = !this.hasAttribute('no-google');
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

    // Deployments with a Google-only OIDC backend (e.g. the tenant
    // self-service portal, which has no /api/auth/google route at all) pass
    // google-href to point the button elsewhere, and google-status-href to
    // gate its visibility the same way Microsoft's is gated above — default
    // behavior (always-shown, hardcoded href) is unchanged for every
    // existing caller.
    const googleHref = this.getAttribute('google-href') || '/api/auth/google';
    const googleStatusHref = this.getAttribute('google-status-href');
    if (showGoogle && googleStatusHref) {
      try {
        const res = await fetch(googleStatusHref, { credentials: 'same-origin' });
        const data = await res.json();
        showGoogle = Boolean(data?.configured);
      } catch {
        showGoogle = false;
      }
    }

    let oauthSection = '';
    if (showGithub || showGoogle || showMicrosoft) {
      let buttons = '';
      if (showMicrosoft) buttons += `<a href="/api/auth/oidc/login" class="btn-oauth">${icons.microsoft} Continue with Microsoft</a>`;
      if (showGithub) buttons += `<a href="/api/auth/github" class="btn-oauth">${GITHUB_ICON} Continue with GitHub</a>`;
      if (showGoogle) buttons += `<a href="${googleHref}" class="btn-oauth">${icons.google} Continue with Google</a>`;
      // The divider separates the credentials form from the OAuth buttons —
      // with no form above it, a lone "or" reads as a rendering glitch.
      oauthSection = `
        ${showCredentials ? '<div class="divider">or</div>' : ''}
        <div class="oauth-section">${buttons}</div>
      `;
    }

    this.innerHTML = `
      <div class="card">
        <div class="brand">
          <div class="brand-icon">${LOGO_ICON}</div>
        </div>
        <h1 class="login-title">Sign in to ${brandTitle}</h1>
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
