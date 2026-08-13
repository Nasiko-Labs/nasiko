import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import { renderMarkdown } from '/common/utils/markdown.js';
import { readA2aStream, frameRenderer, nearBottom } from '/common/utils/a2a-stream.js';
import { usageChipsHtml } from '/common/utils/usage-chips.js';
import { transcribeBlob } from '/common/utils/voice-utils.js';
import '/common/components/voice-input.js';
import '/common/components/agent-steps.js';
import '/common/components/app-module-nav.js';

window.transcribeAudio = transcribeBlob;

import styles from './orchestrator-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class OrchestratorPage extends HTMLElement {
  #initialized = false;
  #sessionId = null;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <app-module-nav module="orchestrator"></app-module-nav>
      <div class="hero-icon" aria-hidden="true">${icons.route('', 24)}</div>
      <h1 class="title">Orchestrate a task</h1>
      <p class="subtitle">Describe a task and Nasiko will orchestrate the right agents to execute it</p>
      <div class="recent-agents" id="recent-agents">
        <div class="recent-agents-grid" id="recent-agents-grid">
          <div class="agent-card-skel"></div>
          <div class="agent-card-skel"></div>
          <div class="agent-card-skel"></div>
        </div>
      </div>
      <div class="messages" id="messages"></div>
      <div class="input-wrap">
        <voice-input
          id="voice-input"
          placeholder="Describe the task you want to execute..."
          transcription-callback="transcribeAudio"
        ></voice-input>
      </div>
      <a class="wf-banner" href="/workflow-new.html">
        <span class="wf-banner-icon" aria-hidden="true">${icons.workflow('', 20)}</span>
        <span class="wf-banner-text">
          <span class="wf-banner-title">Need multiple coordinated steps or agents?</span>
          <span class="wf-banner-sub">Create a workflow to structure complex tasks and reusable operations.</span>
        </span>
        <span class="wf-banner-cta">Create workflow</span>
      </a>
    `;

    this.#loadRecentAgents();

    const voiceInput = this.querySelector('#voice-input');
    const messagesEl = this.querySelector('#messages');

    // Copy code blocks (delegated on messages container)
    messagesEl.addEventListener('click', (e) => {
      const copyBtn = e.target.closest('.md-code-copy');
      if (copyBtn) {
        const codeEl = copyBtn.closest('.md-code-block')?.querySelector('code');
        if (codeEl) {
          navigator.clipboard.writeText(codeEl.textContent).catch(() => {});
          copyBtn.innerHTML = icons.check('', 14);
          setTimeout(() => { copyBtn.innerHTML = icons.copy('', 14); }, 1500);
        }
        return;
      }

      const msgCopyBtn = e.target.closest('.msg-action-copy');
      if (msgCopyBtn) {
        const row = msgCopyBtn.closest('.msg-row');
        const msgEl = row?.querySelector('.msg, .stream-content');
        if (msgEl) {
          navigator.clipboard.writeText(msgEl.textContent).catch(() => {});
          msgCopyBtn.innerHTML = icons.check('', 14);
          setTimeout(() => { msgCopyBtn.innerHTML = icons.copy('', 14); }, 1500);
        }
      }
    });

    voiceInput.addEventListener('voice-input-submit', async (e) => {
      const { value: content, files } = e.detail;
      if (!content && files.length === 0) return;

      voiceInput.reset();
      voiceInput.setLoading(true);
      this.classList.add('has-response');

      // Append user message
      this.#appendMsg(messagesEl, 'user', content);

      // Typing indicator
      const pendingRow = document.createElement('div');
      pendingRow.className = 'msg-row is-assistant';
      pendingRow.innerHTML = `<div class="typing-indicator" aria-label="Agent is responding"><span></span><span></span><span></span></div>`;
      messagesEl.appendChild(pendingRow);
      messagesEl.scrollTop = messagesEl.scrollHeight;

      try {
        // Create session on first message, reuse for subsequent ones
        if (!this.#sessionId) {
          const sessionRes = await apiFetch('/chat/sessions', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ first_prompt: content.slice(0, 100) }),
          });
          if (!sessionRes.ok) throw new Error('Failed to create session');
          const sessionBody = await sessionRes.json();
          const session = sessionBody.data || sessionBody;
          this.#sessionId = session.session_id || session.id;
        }

        // Persist user message
        if (this.#sessionId) {
          apiFetch(`/chat/sessions/${this.#sessionId}/messages`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ role: 'user', content }),
          }).catch(() => {});
        }

        const body = {
          jsonrpc: '2.0',
          id: (crypto.randomUUID ? crypto.randomUUID() : Math.random().toString(36).slice(2) + Date.now().toString(36)),
          method: 'message/stream',
          params: {
            message: {
              messageId: (crypto.randomUUID ? crypto.randomUUID() : Math.random().toString(36).slice(2) + Date.now().toString(36)),
              role: 'ROLE_USER',
              parts: [{ text: content }],
              contextId: this.#sessionId || undefined,
            },
            metadata: this.#sessionId ? { session_id: this.#sessionId } : undefined,
          },
        };

        const res = await apiFetch('/orchestrator/a2a', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });
        if (!res.ok) throw new Error(await res.text());

        pendingRow.remove();
        await this.#readStream(res, messagesEl);
        // Assistant reply is persisted server-side by the orchestrator dispatch
        // (insert_assistant_message in a2a_dispatch.rs) — no client-side write
        // needed, unlike the agent chat page whose direct-agent path does not.
      } catch (err) {
        pendingRow.remove();
        this.#appendMsg(messagesEl, 'assistant', `Error: ${err.message}`);
      } finally {
        voiceInput.setLoading(false);
      }
    });
  }

  #appendMsg(messagesEl, role, content, { usage = null, traceId = null } = {}) {
    const isUser = role === 'user';
    const roleClass = isUser ? 'is-user' : 'is-assistant';

    const row = document.createElement('div');
    row.className = `msg-row ${roleClass}`;

    const div = document.createElement('div');
    div.className = `msg ${roleClass}${isUser ? '' : ' md-body'}`;

    if (isUser) {
      div.textContent = content;
    } else {
      div.innerHTML = renderMarkdown(content);
    }

    row.appendChild(div);

    if (!isUser) {
      const actions = document.createElement('div');
      actions.className = 'msg-actions';
      actions.innerHTML = `
        <button type="button" class="msg-action-copy" aria-label="Copy message" title="Copy">${icons.copy('', 14)}</button>
        ${usageChipsHtml(usage)}
        ${this.#traceLinkHtml(traceId)}
      `;
      row.appendChild(actions);
    }

    messagesEl.appendChild(row);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  #traceLinkHtml(traceId) {
    if (!traceId) return '';
    const q = new URLSearchParams({ trace_id: traceId });
    if (this.#sessionId) q.set('session_id', this.#sessionId);
    return `<a class="msg-action-trace" href="/observability-session.html?${q}"
      aria-label="View trace" title="View trace">${icons.trace('', 14)}<span>Detailed trace</span></a>`;
  }

  async #loadRecentAgents() {
    const grid = this.querySelector('#recent-agents-grid');
    try {
      const res = await apiFetch('/agents?status=running&limit=6');
      if (!res.ok) throw new Error('Failed to fetch');
      const body = await res.json();
      const agents = Array.isArray(body) ? body : (body.data || []);

      if (!agents.length) {
        grid.innerHTML = `<span style="font-size:var(--font-size-sm);color:var(--color-text-muted)">No agents running</span>`;
        return;
      }

      grid.innerHTML = agents.map(agent => {
        const displayName = agent.display_name || agent.name || agent.id;
        return `
          <a class="agent-card" href="/chat.html?agent_name=${encodeURIComponent(agent.name)}&agent_id=${encodeURIComponent(agent.id)}">
            <div class="agent-card-top">
              <span class="agent-card-name">${this.#esc(displayName)}</span>
              <span class="agent-card-go">${icons.arrowUpRight('', 14)}</span>
            </div>
            ${agent.description ? `<div class="agent-card-desc">${this.#esc(agent.description)}</div>` : ''}
          </a>
        `;
      }).join('');
    } catch {
      grid.innerHTML = '';
      this.querySelector('#recent-agents')?.remove();
    }
  }

  async #readStream(res, messagesEl) {
    const streamRow = document.createElement('div');
    streamRow.className = 'msg-row is-assistant';
    const streamArea = document.createElement('div');
    streamArea.className = 'assistant-stream';

    const stepsEl = document.createElement('agent-steps');

    const contentEl = document.createElement('div');
    contentEl.className = 'stream-content md-body';

    const typingEl = document.createElement('div');
    typingEl.className = 'typing-indicator';
    typingEl.setAttribute('aria-label', 'Agent is responding');
    typingEl.innerHTML = '<span></span><span></span><span></span>';

    streamArea.appendChild(stepsEl);
    streamArea.appendChild(typingEl);
    streamArea.appendChild(contentEl);
    streamRow.appendChild(streamArea);
    messagesEl.appendChild(streamRow);
    messagesEl.scrollTop = messagesEl.scrollHeight;

    const follow = () => {
      if (nearBottom(messagesEl)) messagesEl.scrollTop = messagesEl.scrollHeight;
    };

    const showContent = (html, { progress = false } = {}) => {
      typingEl.remove();
      contentEl.classList.add('is-visible');
      contentEl.classList.toggle('is-progress', progress);
      contentEl.innerHTML = html;
      follow();
    };

    const renderReply = frameRenderer((text) => {
      stepsEl.finish();
      showContent(renderMarkdown(text));
    });
    const out = await readA2aStream(res, {
      onReply: renderReply,
      onActivity: (line) => {
        stepsEl.onActivity(line);
        follow();
      },
      onData: (d) => {
        stepsEl.onEvent(d);
        follow();
      },
      onError: (message) => {
        stepsEl.finish();
        showContent(`<span style="color:var(--color-error)">${this.#esc(message)}</span>`);
      },
    });

    stepsEl.finish();
    typingEl.remove();
    let fullText = out.text;
    if (out.failed && !fullText) {
      fullText = out.errorMessage;
      showContent(`<span style="color:var(--color-error)">${this.#esc(fullText)}</span>`);
    } else if (!fullText) {
      showContent(renderMarkdown('No response'));
      fullText = 'No response';
    } else {
      showContent(renderMarkdown(fullText));
    }

    // Add actions to stream row
    const actions = document.createElement('div');
    actions.className = 'msg-actions';
    actions.innerHTML = `
      <button type="button" class="msg-action-copy" aria-label="Copy message" title="Copy">${icons.copy('', 14)}</button>
      ${usageChipsHtml(out.usage)}
      ${this.#traceLinkHtml(out.traceId)}
    `;
    streamArea.appendChild(actions);

    return { text: fullText, traceId: out.traceId, usage: out.usage };
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('orchestrator-page', OrchestratorPage);