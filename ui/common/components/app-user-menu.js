/**
 * User avatar dropdown with account display, account switching, theme
 * selection, and logout.
 *
 * @element app-user-menu
 * @attr {string} current-user - JSON `{ name, email, avatar }` for the logged-in user
 * @fires user-add-account - "Add account" clicked — bubbles
 * @fires user-logout - "Logout" clicked — bubbles
 */
import { icons } from "../utils/icons.js";
import { getTheme, setTheme } from "../utils/theme.js";
const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (app-user-menu) {
    :scope { display: block; }
    .user-button {
      justify-content: var(--user-btn-justify, center);
      width: var(--user-btn-w, 36px);
      height: var(--user-btn-h, 36px);
      padding: var(--user-btn-padding, 0);
      border: 2px solid var(--color-border);
      border-radius: var(--radius-md);
      &:hover {
        border-color: var(--color-primary);
        box-shadow: 0 0 0 4px color-mix(in srgb, var(--color-primary) 18%, transparent);
      }
      &:active {
        border-color: var(--color-primary);
        background: color-mix(in srgb, var(--color-primary) 15%, transparent);
        box-shadow: 0 0 0 4px color-mix(in srgb, var(--color-primary) 30%, transparent);
      }
      &:focus { box-shadow: 0 0 0 2px var(--color-primary-ring); }
      &.is-open {
        border-color: var(--color-primary);
        box-shadow: 0 0 0 4px color-mix(in srgb, var(--color-primary) 22%, transparent);
        background: color-mix(in srgb, var(--color-primary) 10%, transparent);
      }
      & svg:last-child { display: none; }

      @media (min-width: 1024px) {
        gap: var(--space-sm);
        border: 1px solid transparent;
        border-radius: var(--r-10);
        &:hover {
          border-color: transparent;
          background: light-dark(var(--cream-50), var(--neutral-800));
          box-shadow: none;
        }
        &:active, &.is-open {
          border-color: var(--border-canvas);
          background: light-dark(var(--cream-50), var(--neutral-800));
          box-shadow: none;
        }
        & svg:last-child {
          display: var(--user-name-display, block);
          margin-left: auto;
          flex-shrink: 0;
          color: var(--color-text-muted);
        }
      }
    }
    .user-avatar {
      width: 100%;
      height: 100%;
      border-radius: calc(var(--radius-md) - 2px);
      background: var(--color-primary);
      color: var(--color-on-primary);
      display: flex;
      align-items: center;
      justify-content: center;
      font-weight: 700;
      font-size: var(--font-size-sm);

      @media (min-width: 1024px) {
        width: var(--user-avatar-size, 34px);
        height: var(--user-avatar-size, 34px);
        flex-shrink: 0;
        border-radius: var(--user-avatar-radius, var(--radius-full, 50%));
      }
    }
    .user-info {
      display: none;
      @media (min-width: 1024px) {
        display: var(--user-name-display, flex);
        flex-direction: column;
        justify-content: center;
        gap: 1px;
        flex: 1;
        min-width: 0;
        text-align: left;
      }
    }
    .user-name {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-size: var(--font-size-sm);
      font-weight: 600;
      line-height: 1.2;
      color: var(--color-text-main);
    }
    .user-email {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-size: var(--font-size-xs);
      line-height: 1.2;
      color: var(--color-text-muted);
    }
    .user-dropdown {
      position: absolute;
      top: var(--user-dropdown-top, calc(100% + var(--space-xs)));
      bottom: var(--user-dropdown-bottom, unset);
      right: var(--user-dropdown-right, 0);
      left: var(--user-dropdown-left, unset);
      background: var(--color-bg-surface);
      border: 1px solid color-mix(in srgb, var(--color-border) 100%, var(--color-text-muted) 40%);
      border-radius: var(--radius-md);
      box-shadow: var(--shadow-md), var(--shadow-xl);
      min-width: min(280px, calc(100vw - 2rem));
      max-width: 320px;
      max-height: min(400px, calc(100dvh - 6rem));
      overflow: hidden;
      z-index: 1000;
      display: none;
      &.is-visible { display: block; }

      @media (min-width: 1024px) {
        min-width: 18rem;
        width: min(20rem, calc(100vw - var(--app-sidebar-width) - var(--space-lg) - var(--space-lg)));
        max-width: min(22rem, calc(100vw - var(--app-sidebar-width) - var(--space-lg) - var(--space-lg)));
        max-height: min(32rem, calc(100dvh - var(--space-lg) - var(--space-lg)));
      }
    }
    .dropdown-header {
      padding: var(--space-md);
      border-bottom: 1px solid var(--color-border);

      @media (min-width: 1024px) {
        padding: var(--space-sm) var(--space-md);
      }
    }
    .dropdown-title {
      font-size: var(--font-size-xs);
      font-weight: 500;
      color: var(--color-text-muted);
      text-transform: uppercase;
      letter-spacing: 0.05em;
    }
    .user-list {
      padding: var(--space-xs);
      margin: 0;
      list-style: none;
      overflow-y: auto;

      & li { padding: 0; margin: 0; }

      @media (min-width: 1024px) {
        max-height: min(22rem, calc(100dvh - 10rem));
      }
    }
    .user-item {
      color: var(--color-text-main);
      display: flex;
      align-items: center;
      gap: var(--space-sm);
      padding: var(--space-sm) var(--space-md);
      border-radius: var(--radius-sm);
      border-left: 3px solid transparent;
      &:hover {
        color: var(--color-text-main);
        background: color-mix(in srgb, var(--color-primary) 8%, transparent);
        border-left-color: var(--color-primary);
      }
      &:active { background: color-mix(in srgb, var(--color-primary) 15%, transparent); }
      &.is-active {
        background: color-mix(in srgb, var(--color-primary) 10%, transparent);
        border-left-color: var(--color-primary);
        & .user-item-name { color: var(--color-primary); font-weight: 700; }
        & .user-item-email { color: var(--color-text-muted); }
      }
    }
    .user-item-info { flex: 1; min-width: 0; }
    .user-item-name { font-weight: 500; font-size: var(--font-size-sm); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .user-item-email { font-size: var(--font-size-xs); color: var(--color-text-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .user-item-check { flex-shrink: 0; width: 16px; height: 16px; color: var(--color-primary); }
    .dropdown-theme {
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: var(--space-sm);
      padding: var(--space-sm) var(--space-md);
      border-top: 1px solid var(--color-border);
    }
    .theme-title {
      font-size: var(--font-size-xs);
      font-weight: 500;
      color: var(--color-text-muted);
      text-transform: uppercase;
      letter-spacing: 0.05em;
    }
    .theme-switch {
      display: inline-flex;
      gap: 2px;
      padding: 2px;
      border: 1px solid var(--color-border);
      border-radius: var(--radius-md);
      background: var(--color-bg-base);
    }
    .theme-option {
      width: 32px;
      height: 26px;
      border-radius: calc(var(--radius-md) - 3px);
      color: var(--color-text-muted);
      &:hover { color: var(--color-text-main); }
      &:focus-visible { box-shadow: 0 0 0 2px var(--color-primary-ring); }
      &[aria-pressed="true"] {
        background: var(--color-bg-surface);
        color: var(--color-text-main);
        box-shadow: var(--shadow-sm);
      }
    }
    .dropdown-footer { padding: var(--space-xs); border-top: 1px solid var(--color-border); }
    .dropdown-button {
      width: 100%;
      padding: var(--space-sm) var(--space-md);
      border-radius: var(--radius-sm);
      text-align: left;
      font-size: var(--font-size-sm);
      color: var(--color-text-main);
      &:hover { background: var(--color-bg-base); }
      &.is-add { color: var(--color-primary); border-color: color-mix(in srgb, var(--color-primary) 40%, transparent); margin-bottom: var(--space-xs); &:hover { background: color-mix(in srgb, var(--color-primary) 8%, transparent); } }
      &.is-danger { color: var(--color-error); border-color: color-mix(in srgb, var(--color-error) 30%, transparent); &:hover { background: color-mix(in srgb, var(--color-error) 10%, transparent); } }
    }
    .btn-icon { flex-shrink: 0; width: 14px; height: 14px; }
  }
`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];



const IC_CHECK = `<svg class="user-item-check" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="20 6 9 17 4 12"/></svg>`;

/** User account dropdown for app-header. */
export class AppUserMenu extends HTMLElement {
  #users = [];
  #visible = false;
  #onDocClick = (e) => { if (!this.contains(e.target)) this.hide(); };

  #setOpenState(open) {
    this.#visible = open;
    const btn = this.querySelector('[data-user-toggle]');
    const dropdown = this.querySelector('[data-user-dropdown]');
    btn?.classList.toggle('is-open', open);
    dropdown?.classList.toggle('is-visible', open);
  }

  static get observedAttributes() {
    return ['current-user'];
  }

  attributeChangedCallback(_name, _old, _new) {
    if (this.isConnected) this.#render();
  }

  set users(v) {
    this.#users = v;
    if (this.isConnected) this.#render();
  }

  get users() { return this.#users; }

  connectedCallback() {
    this.#render();
    document.addEventListener('click', this.#onDocClick);
  }

  disconnectedCallback() {
    document.removeEventListener('click', this.#onDocClick);
  }

  hide() {
    this.#setOpenState(false);
  }

  #render() {
    const currentUser = this.getAttribute('current-user');
    const initial = currentUser ? currentUser.charAt(0).toUpperCase() : 'U';
    const email = this.#users.find(u => u.username === currentUser)?.email || '';
    const eff = window.location.pathname.replace(/^\/u\/[^/]+/, '') || '/';

    this.innerHTML = `
      <button class="user-button" data-user-toggle type="button"
        aria-label="User menu${currentUser ? ` — ${currentUser}` : ''}">
        <div class="user-avatar">${initial}</div>
        <span class="user-info">
          <span class="user-name">${currentUser || ''}</span>
          ${email ? `<span class="user-email">${email}</span>` : ''}
        </span>
        ${icons.moreVertical('', 16)}
      </button>
      <div class="user-dropdown" data-user-dropdown>
        <div class="dropdown-header">
          <div class="dropdown-title">Accounts</div>
        </div>
        <ul class="user-list">
          ${this.#users.map(user => {
            const isActive = user.username === currentUser;
            return `
            <li>
              <a class="user-item ${isActive ? 'is-active' : ''}"
                 href="/u/${user.username}${eff}">
                <div class="user-item-info">
                  <div class="user-item-name">${user.username}</div>
                  ${user.email ? `<div class="user-item-email">${user.email}</div>` : ''}
                </div>
                ${isActive ? IC_CHECK : ''}
              </a>
            </li>`;
          }).join('')}
        </ul>
        <div class="dropdown-theme">
          <span class="theme-title" id="app-theme-title">Theme</span>
          <div class="theme-switch" role="group" aria-labelledby="app-theme-title">
            ${[
              ['light', 'sun', 'Light theme'],
              ['dark', 'moon', 'Dark theme'],
              ['system', 'monitor', 'Follow system theme'],
            ].map(([value, icon, label]) => `
              <button class="theme-option" type="button" data-theme-choice="${value}"
                aria-pressed="${getTheme() === value}" aria-label="${label}" title="${label}">
                ${icons[icon]('', 15)}
              </button>`).join('')}
          </div>
        </div>
        <div class="dropdown-footer">
          <button class="dropdown-button is-add" data-add-account>
            ${icons.plus('btn-icon', 14)} Add Account
          </button>
          <button class="dropdown-button is-danger" data-logout>
            ${icons.logOut('btn-icon', 14)} Sign Out
          </button>
        </div>
      </div>
    `;

    this.querySelector('[data-user-toggle]')?.addEventListener('click', e => {
      e.stopPropagation();
      this.#visible ? this.hide() : this.#showDropdown();
    });

    this.querySelector('[data-add-account]')?.addEventListener('click', () => {
      this.dispatchEvent(new CustomEvent('user-add-account', { bubbles: true }));
    });

    this.querySelector('[data-logout]')?.addEventListener('click', () => {
      this.dispatchEvent(new CustomEvent('user-logout', { bubbles: true }));
    });

    // Theme choice applies instantly and keeps the dropdown open so the
    // switch is visible under the new theme.
    const themeButtons = this.querySelectorAll('[data-theme-choice]');
    themeButtons.forEach(btn => btn.addEventListener('click', () => {
      setTheme(btn.dataset.themeChoice);
      themeButtons.forEach(b => b.setAttribute('aria-pressed', String(b === btn)));
    }));
  }

  #showDropdown() {
    this.#setOpenState(true);
  }
}

customElements.define('app-user-menu', AppUserMenu);
