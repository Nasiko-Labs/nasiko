import { icons } from '/common/utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (agents-page) {
  :scope {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: var(--space-2xl) var(--space-md);
  }
  .title {
    font-size: var(--font-size-3xl);
    font-weight: 400;
    color: var(--color-text-main);
    margin-bottom: var(--space-lg);
    text-align: center;
  }
  .search-wrap {
    width: min(100%, 600px);
    position: relative;
    margin-bottom: var(--space-lg);
  }
  .search-wrap .icon {
    position: absolute;
    left: 14px;
    top: 50%;
    transform: translateY(-50%);
    color: var(--color-text-muted);
    pointer-events: none;
  }
  .search-wrap:focus-within .icon { color: var(--color-primary); }
  .search-wrap input {
    width: 100%;
    padding: var(--space-sm) var(--space-md) var(--space-sm) 42px;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    font-size: var(--font-size-base);
    color: var(--color-text-main);
    font-family: inherit;
  }
  .search-wrap input:focus {
    outline: none;
    border-color: var(--color-primary);
    box-shadow: 0 0 0 3px var(--color-primary-ring);
  }
  .search-wrap input::placeholder { color: var(--color-text-muted); }

  .tabs {
    display: flex;
    gap: var(--space-lg);
    border-bottom: 1px solid var(--color-border);
    margin-bottom: var(--space-xl);
    width: 100%;
    max-width: 1200px;
    overflow-x: auto;
    justify-content: center;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: var(--space-sm) var(--space-xs);
    font-size: var(--font-size-sm);
    font-weight: 500;
    color: var(--color-text-muted);
    border: none;
    border-bottom: 2px solid transparent;
    background: none;
    cursor: pointer;
    white-space: nowrap;
    transition: color 0.15s, border-color 0.15s;
  }
  .tab:hover { color: var(--color-text-main); }
  .tab.is-active {
    color: var(--color-primary);
    border-bottom-color: var(--color-primary);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: var(--space-lg);
    width: 100%;
    max-width: 1200px;
  }
  .card {
    border: 1px solid var(--color-border);
    border-radius: var(--radius-lg);
    padding: var(--space-md);
    background: var(--color-bg-surface);
    display: flex;
    flex-direction: column;
    gap: var(--space-xs);
    transition: border-color 0.15s;
  }
  .card:hover { border-color: var(--color-primary); }
  .card-name {
    font-size: var(--font-size-lg);
    font-weight: 600;
    color: var(--color-text-main);
  }
  .card-tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .tag {
    font-size: var(--font-size-xs);
    padding: 2px 10px;
    border-radius: var(--radius-sm);
    background: var(--color-bg-base);
    border: 1px solid var(--color-border);
    color: var(--color-text-muted);
  }
  .card-desc {
    font-size: var(--font-size-sm);
    color: var(--color-text-muted);
    line-height: 1.5;
    flex: 1;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .card-btn {
    margin-top: var(--space-sm);
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-sm) var(--space-md);
    border: 1px solid var(--color-primary);
    border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--color-primary) 10%, transparent);
    color: var(--color-text-main);
    font-size: var(--font-size-base);
    font-weight: 500;
    cursor: pointer;
    text-decoration: none;
    transition: background 0.15s;
  }
  .card-btn:hover {
    background: color-mix(in srgb, var(--color-primary) 20%, transparent);
  }
  .empty {
    grid-column: 1 / -1;
    text-align: center;
    color: var(--color-text-muted);
    font-size: var(--font-size-base);
    padding: var(--space-2xl);
    font-style: italic;
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const CATEGORIES = [
  { key: 'all', label: 'For you', icon: icons.settings('', 18) },
  { key: 'hr', label: 'HR', icon: icons.user('', 18) },
  { key: 'devops', label: 'DevOps', icon: icons.code('', 18) },
  { key: 'finance', label: 'Finance', icon: icons.cube('', 18) },
  { key: 'legal', label: 'Legal', icon: icons.document('', 18) },
  { key: 'utilities', label: 'AI infra', icon: icons.layers('', 18) },
];

class AgentsPage extends HTMLElement {
  #agents = [];
  #activeCategory = 'all';

  connectedCallback() {
    this.innerHTML = `
      <h1 class="title">Choose an agent to assist you</h1>
      <div class="search-wrap">
        <span class="icon">${icons.search('', 20)}</span>
        <input type="search" placeholder="Search agents by name, skill, or capability..." />
      </div>
      <nav class="tabs">${CATEGORIES.map(c =>
        `<button class="tab${c.key === 'all' ? ' is-active' : ''}" data-cat="${c.key}">${c.icon}<span>${c.label}</span></button>`
      ).join('')}</nav>
      <div class="grid">${this.#skeletonCards()}</div>
    `;

    this.querySelector('.tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.tab');
      if (!tab) return;
      this.#activeCategory = tab.dataset.cat;
      this.querySelectorAll('.tab').forEach(t => t.classList.remove('is-active'));
      tab.classList.add('is-active');
      this.#renderGrid();
    });

    this.querySelector('input').addEventListener('input', () => this.#renderGrid());


    this.#loadAgents();
  }

  async #loadAgents() {
    const result = await window.fetchAgents('', 1, 100);
    this.#agents = result.data || [];
    this.#renderGrid();
  }

  #skeletonCards() {
    return Array.from({ length: 6 }, () => `
      <div class="card" style="min-height:160px;">
        <div style="width:60%;height:1em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
        <div style="display:flex;gap:6px;margin-top:var(--space-sm);">
          <div style="width:50px;height:1.2em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
          <div style="width:60px;height:1.2em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
          <div style="width:70px;height:1.2em;background:var(--color-border);border-radius:var(--radius-sm);"></div>
        </div>
        <div style="width:90%;height:0.8em;background:var(--color-border);border-radius:var(--radius-sm);margin-top:var(--space-sm);"></div>
        <div style="width:70%;height:0.8em;background:var(--color-border);border-radius:var(--radius-sm);margin-top:4px;"></div>
        <div style="width:100%;height:2.2em;background:var(--color-border);border-radius:var(--radius-md);margin-top:auto;opacity:0.5;"></div>
      </div>
    `).join('');
  }

  #renderGrid() {
    const q = this.querySelector('input').value.toLowerCase();
    let filtered = this.#agents;

    if (this.#activeCategory !== 'all') {
      filtered = filtered.filter(a => (a.tags || []).some(t => t.toLowerCase() === this.#activeCategory));
    }
    if (q) {
      filtered = filtered.filter(a =>
        (a.display_name || a.name || '').toLowerCase().includes(q) ||
        (a.description || '').toLowerCase().includes(q) ||
        (a.tags || []).some(t => t.toLowerCase().includes(q))
      );
    }

    const grid = this.querySelector('.grid');
    if (!filtered.length) {
      grid.innerHTML = '<div class="empty">No agents found.</div>';
      return;
    }

    grid.innerHTML = filtered.map(a => `
      <div class="card">
        <div class="card-name">${a.display_name || a.name}</div>
        <div class="card-tags">${(a.tags || []).map(t => `<span class="tag">${t}</span>`).join('')}</div>
        <div class="card-desc">${a.description || ''}</div>
        <a class="card-btn" href="/chat.html?agent_id=${a.id}&agent_name=${encodeURIComponent(a.display_name || a.name)}">Start session</a>
      </div>
    `).join('');
  }
}

customElements.define('agents-page', AgentsPage);
