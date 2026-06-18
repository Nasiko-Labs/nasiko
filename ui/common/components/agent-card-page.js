import { icons } from '/common/utils/icons.js';

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (agent-card-page) {
  :scope { display: block; }
  .card {
    max-width: 640px;
    margin: 0 auto;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-lg);
    background: var(--color-bg-surface);
    overflow: hidden;
  }
  .card-header {
    padding: var(--space-lg);
    border-bottom: 1px solid var(--color-border);
    display: flex;
    align-items: center;
    gap: var(--space-md);
  }
  .card-avatar {
    width: 48px;
    height: 48px;
    border-radius: 50%;
    background: color-mix(in srgb, var(--color-primary) 15%, transparent);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--color-primary);
    flex-shrink: 0;
  }
  .card-title { font-size: var(--font-size-xl); font-weight: 600; color: var(--color-text-main); }
  .card-subtitle { font-size: var(--font-size-sm); color: var(--color-text-muted); margin-top: 2px; }
  .card-body { padding: var(--space-lg); }
  .card-section { margin-bottom: var(--space-lg); }
  .card-section:last-child { margin-bottom: 0; }
  .card-section-title {
    font-size: var(--font-size-xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--color-text-muted);
    margin-bottom: var(--space-sm);
  }
  .card-desc {
    font-size: var(--font-size-sm);
    color: var(--color-text-main);
    line-height: 1.6;
  }
  .card-tags { display: flex; flex-wrap: wrap; gap: 6px; }
  .card-tag {
    font-size: var(--font-size-xs);
    padding: 3px 10px;
    border-radius: var(--radius-sm);
    background: var(--color-bg-base);
    border: 1px solid var(--color-border);
    color: var(--color-text-muted);
  }
  .card-skills { display: flex; flex-direction: column; gap: var(--space-xs); }
  .card-skill {
    display: flex;
    align-items: baseline;
    gap: var(--space-sm);
    font-size: var(--font-size-sm);
  }
  .card-skill-name { font-weight: 500; font-family: var(--font-mono); font-size: var(--font-size-xs); }
  .card-skill-desc { color: var(--color-text-muted); }
  .card-meta {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-sm);
    font-size: var(--font-size-sm);
  }
  .card-meta dt { color: var(--color-text-muted); font-size: var(--font-size-xs); }
  .card-meta dd { margin: 0; color: var(--color-text-main); font-family: var(--font-mono); font-size: var(--font-size-xs); }
  .card-footer {
    padding: var(--space-md) var(--space-lg);
    border-top: 1px solid var(--color-border);
    background: var(--color-bg-base);
    display: flex;
    gap: var(--space-sm);
    justify-content: flex-end;
  }
  .card-footer a {
    padding: var(--space-xs) var(--space-md);
    border-radius: var(--radius-md);
    font-size: var(--font-size-sm);
    font-weight: 500;
    text-decoration: none;
    border: 1px solid var(--color-primary);
    background: color-mix(in srgb, var(--color-primary) 10%, transparent);
    color: var(--color-text-main);
  }
  .card-footer a:hover { background: color-mix(in srgb, var(--color-primary) 20%, transparent); }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class AgentCardPage extends HTMLElement {
  connectedCallback() {
    const agentId = new URLSearchParams(location.search).get('id');
    if (!agentId) {
      this.innerHTML = '<p style="color:var(--color-text-muted);">No agent ID specified.</p>';
      return;
    }
    this.innerHTML = '<app-skeleton height="400px" style="max-width:640px;margin:0 auto;"></app-skeleton>';
    this.#load(agentId);
  }

  async #load(agentId) {
    const card = await window.fetchAgentCard(agentId);
    if (!card) {
      this.innerHTML = '<p style="color:var(--color-error);">Agent card not found.</p>';
      return;
    }
    this.#render(card);
  }

  #render(c) {
    const skillsHtml = (c.skills || []).map(s => `
      <div class="card-skill">
        <span class="card-skill-name">${s.name}</span>
        <span class="card-skill-desc">— ${s.description || ''}</span>
      </div>
    `).join('');

    this.innerHTML = `
      <div class="card">
        <div class="card-header">
          <div class="card-avatar">${icons.cube('', 24)}</div>
          <div>
            <div class="card-title">${c.display_name || c.name}</div>
            <div class="card-subtitle">${c.name} · v${c.version || '?'}</div>
          </div>
        </div>

        <div class="card-body">
          <div class="card-section">
            <div class="card-section-title">Description</div>
            <div class="card-desc">${c.description || '—'}</div>
          </div>

          ${c.tags?.length ? `
          <div class="card-section">
            <div class="card-section-title">Tags</div>
            <div class="card-tags">${c.tags.map(t => `<span class="card-tag">${t}</span>`).join('')}</div>
          </div>
          ` : ''}

          ${skillsHtml ? `
          <div class="card-section">
            <div class="card-section-title">Skills</div>
            <div class="card-skills">${skillsHtml}</div>
          </div>
          ` : ''}

          <div class="card-section">
            <div class="card-section-title">Metadata</div>
            <dl class="card-meta">
              <div><dt>Image</dt><dd>${c.image || '—'}</dd></div>
              <div><dt>Port</dt><dd>${c.port || '—'}</dd></div>
              <div><dt>Status</dt><dd>${c.status || '—'}</dd></div>
              <div><dt>Version</dt><dd>${c.version || '—'}</dd></div>
            </dl>
          </div>
        </div>

        <div class="card-footer">
          <a href="/agent.html?id=${c.id}">Details</a>
          <a href="/chat.html?agent_id=${c.id}">Start Chat</a>
        </div>
      </div>
    `;
  }
}

customElements.define('agent-card-page', AgentCardPage);
