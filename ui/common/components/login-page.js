import { icons } from '../utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (login-page) {
  :scope {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 100dvh;
    width: 100%;
  }
  .card {
    width: min(100% - 2rem, 380px);
    padding: var(--space-2xl) var(--space-xl);
  }
  .brand {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-sm);
    margin-bottom: var(--space-xl);
  }
  .brand-icon {
    width: 36px;
    height: 36px;
    background: var(--color-primary);
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .brand-icon svg { width: 20px; height: 20px; fill: white; }
  .brand-text {
    font-size: var(--font-size-xl);
    font-weight: 600;
    color: var(--color-text-main);
  }
  .subtitle {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    text-align: center;
    margin-bottom: var(--space-lg);
  }
  form { display: flex; flex-direction: column; gap: var(--space-sm); }
  .field { display: flex; flex-direction: column; gap: 3px; }
  label {
    font-size: var(--font-size-xs);
    font-weight: 500;
    color: var(--color-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  input {
    width: 100%;
    padding: var(--space-xs) var(--space-sm);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    font-family: inherit;
  }
  input:focus {
    outline: none;
    border-color: var(--color-primary);
    box-shadow: 0 0 0 3px var(--color-primary-ring);
  }
  .btn-submit {
    width: 100%;
    padding: var(--space-sm);
    background: var(--color-primary);
    color: var(--color-on-primary);
    font-weight: 500;
    font-size: var(--font-size-sm);
    border-radius: var(--radius-md);
    cursor: pointer;
    border: none;
    margin-top: var(--space-xs);
  }
  .btn-submit:hover { background: var(--color-primary-hover); }
  .btn-submit:disabled { opacity: 0.6; cursor: not-allowed; }
  .error-msg {
    color: var(--color-error);
    font-size: var(--font-size-xs);
    display: none;
  }
  .error-msg.visible { display: block; }
  .divider {
    display: flex;
    align-items: center;
    gap: var(--space-md);
    margin: var(--space-md) 0;
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
  }
  .divider::before, .divider::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--color-border);
  }
  .oauth-section { display: flex; flex-direction: column; gap: var(--space-xs); }
  .btn-oauth {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-sm);
    width: 100%;
    padding: var(--space-xs) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-base);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    font-weight: 500;
    cursor: pointer;
    text-decoration: none;
    transition: border-color 0.15s;
  }
  .btn-oauth:hover { border-color: var(--color-text-muted); }
  .btn-oauth svg { width: 18px; height: 18px; flex-shrink: 0; }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const GITHUB_ICON = `<svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>`;
const LOGO_ICON = `<svg viewBox="0 0 24 24"><path d="M3 3h18v18H3V3zm2 2v14h14V5H5zm3 3h2v8H8V8zm3 2h2v6h-2v-6zm3-1h2v7h-2V9z"/></svg>`;

class LoginPage extends HTMLElement {
  connectedCallback() {
    const brandTitle = this.getAttribute('brand-title') || 'Nasiko';
    const subtitle = this.getAttribute('subtitle') || 'Sign in to your workspace';
    const showGithub = !this.hasAttribute('no-github');
    const showGoogle = !this.hasAttribute('no-google');
    const showCredentials = !this.hasAttribute('no-credentials');

    let oauthSection = '';
    if (showGithub || showGoogle) {
      let buttons = '';
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
