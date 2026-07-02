import { icons } from '/common/utils/icons.js';
import styles from './agent-card-page.css' with { type: 'css' };
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
