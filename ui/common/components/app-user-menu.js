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
      border: 1px solid transparent;
      border-radius: var(--r-8);
      /* DS: quiet fill on hover, gold ring on :focus-visible only — the old
         gold glow rings on hover/active/open were this component's own
         invention and read as a different system to everything around it. */
      &:hover { background: var(--bg-surface-hover); }
      &:active, &.is-open {
        background: var(--bg-surface-hover);
        border-color: var(--color-border);
      }
      &:focus-visible {
        outline: none;
        box-shadow: 0 0 0 2px var(--color-primary-ring);
      }
      & svg:last-child { display: none; }

      @media (min-width: 1024px) {
        gap: var(--space-sm);
        border: 1px solid transparent;
        border-radius: var(--r-10);
        &:hover {
          border-color: transparent;
          background: var(--bg-input);
          box-shadow: none;
        }
        &:active, &.is-open {
          border-color: var(--border-canvas);
          background: var(--bg-input);
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
      border-radius: var(--r-6);
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
      /* Hosts inside overflow-clipped chrome (the rail) switch this to fixed
         and position against the viewport via the vars below. */
      position: var(--user-dropdown-position, absolute);
      top: var(--user-dropdown-top, calc(100% + var(--space-xs)));
      bottom: var(--user-dropdown-bottom, unset);
      right: var(--user-dropdown-right, 0);
      left: var(--user-dropdown-left, unset);
      background: var(--color-bg-surface);
      /* Dark needs the stronger edge: the panel and the content plane are both
         --color-bg-surface there, so the default hairline leaves the flyout
         indistinguishable from the page behind it. */
      border: 1px solid light-dark(var(--color-border), var(--neutral-700));
      /* r12 + the single menu shadow token, like every other floating DS
         surface; the doubled md+xl stack read as a heavier, foreign card. */
      border-radius: var(--r-12);
      box-shadow: var(--shadow-md);
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
    /* Sentence case, no tracking — the reskin dropped uppercase micro-labels
       everywhere else (table headers, module-nav group headings). */
    .dropdown-title {
      font-size: var(--font-size-xs);
      font-weight: 600;
      color: var(--color-text-muted);
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
    /* Selected row = sand-100 fill at r8, the same active treatment the module
       tree nav uses. The old 3px gold left-border + gold bold name was a
       marker style used nowhere else in the product. */
    .user-item {
      color: var(--color-text-main);
      display: flex;
      align-items: center;
      gap: var(--space-sm);
      padding: var(--space-sm) var(--space-md);
      border-radius: var(--r-8);
      &:hover { color: var(--color-text-main); background: var(--bg-input); }
      &.is-active {
        background: var(--bg-surface-hover);
        & .user-item-name { font-weight: 600; }
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
      font-weight: 600;
      color: var(--color-text-muted);
    }
    .theme-switch {
      display: inline-flex;
      gap: 2px;
      padding: 2px;
      border: 1px solid var(--color-border);
      border-radius: var(--r-8);
      background: var(--bg-input);
    }
    .theme-option {
      width: 32px;
      height: 26px;
      border-radius: var(--r-6);
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
      /* justify-content + gap, not text-align: global.css's bare button reset
         sets display:inline-flex with justify-content:center, so text-align
         could never left-align these — the label sat centred and the icon
         jammed against it with no gap. */
      justify-content: flex-start;
      gap: var(--space-sm);
      padding: var(--space-sm) var(--space-md);
      border-radius: var(--r-8);
      font-size: var(--font-size-sm);
      color: var(--color-text-main);
      &:hover { background: var(--bg-input); }
      /* Both read as quiet menu rows matching the account rows above. "Add
         Account" was gold text (weak contrast on the sand plane, and gold is
         reserved for selection/accent, not for a routine action); only the
         destructive action keeps a colour, on its own hover tint. */
      &.is-danger {
        color: var(--color-error);
        &:hover { background: var(--color-error-bg); }
      }
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
          <button class="dropdown-button" data-add-account>
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
