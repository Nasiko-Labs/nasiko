import { icons } from '/common/utils/icons.js';
import '/common/components/secrets-manager.js';

import styles from './secrets-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

// Workspace-level secrets. All CRUD lives in <secrets-manager scope="user">
// (GET|POST /api/secrets, DELETE /api/secrets/{name}); this page only supplies
// the page chrome around it.
class SecretsPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;
    this.innerHTML = `
      <header class="page-head">
        <h1 class="title-page">Secrets</h1>
        <p class="page-sub">API credentials stored in this workspace. Router configs and agents reference secrets by name.</p>
      </header>
      <secrets-manager scope="user"></secrets-manager>
      <div class="note-well">${icons.lock('note-icon', 16)}<span>Keys are write-only. Once saved, a secret can be rotated or deleted but never read back — configs reference it by name.</span></div>
    `;
  }
}

customElements.define('secrets-page', SecretsPage);
