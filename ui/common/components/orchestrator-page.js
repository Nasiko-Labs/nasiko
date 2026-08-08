import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import { renderMarkdown } from '/common/utils/markdown.js';
import { readA2aStream, frameRenderer } from '/common/utils/a2a-stream.js';
import { usageChipsHtml } from '/common/utils/usage-chips.js';
import '/common/components/voice-input.js';
import '/common/components/agent-steps.js';

window.transcribeAudio = async (blob) => {
  const form = new FormData();
  form.append('file', blob, 'audio.webm');
  const res = await apiFetch('/transcribe', { method: 'POST', body: form });
  if (!res.ok) throw new Error(await res.text());
  const data = await res.json();
  return data.text;
};

import styles from './orchestrator-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class OrchestratorPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <div class="hero-icon" aria-hidden="true">${icons.route('', 24)}</div>
      <h1 class="title">Orchestrate a task</h1>
      <p class="subtitle">Describe a task and Nasiko will orchestrate the right agents to execute it.</p>
      <div class="recent-agents" id="recent-agents">
        <div class="recent-agents-grid" id="recent-agents-grid">
          <div class="agent-card-skel"></div>
          <div class="agent-card-skel"></div>
          <div class="agent-card-skel"></div>
        </div>
      </div>
      <div class="input-wrap">
        <voice-input
          id="voice-input"
          placeholder="Describe the task you want to execute..."
          transcription-callback="transcribeAudio"
        ></voice-input>
      </div>
      <div class="response-area" id="response-area">
        <div class="steps-slot" id="steps-slot"></div>
        <div class="response-wrap">
          <div class="typing-indicator" id="response-typing" style="display:none;" aria-label="Agent is responding"><span></span><span></span><span></span></div>
          <div class="response-content md-body" id="response-content"></div>
          <div class="msg-actions" id="response-actions" style="display:none;">
            <button type="button" class="msg-action-copy" aria-label="Copy response" title="Copy">${icons.copy('', 14)}</button>
            <a class="msg-action-trace" id="response-trace" style="display:none;" href="#" aria-label="View trace" title="View trace">${icons.trace('', 14)}</a>
          </div>
        </div>
        <div class="response-usage" id="response-usage"></div>
        <a class="continue-link" id="continue-link" style="display:none;" href="#">Continue in chat</a>
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
    const responseArea = this.querySelector('#response-area');
    const stepsSlot = this.querySelector('#steps-slot');
    const responseContent = this.querySelector('#response-content');
    const responseActions = this.querySelector('#response-actions');
    const responseTrace = this.querySelector('#response-trace');
    const continueLink = this.querySelector('#continue-link');
    continueLink.insertAdjacentHTML('beforeend', ' ' + icons.chevronRight('', 14));

    // Copy full response
    responseActions.querySelector('.msg-action-copy').addEventListener('click', (e) => {
      const btn = e.currentTarget;
      navigator.clipboard.writeText(responseContent.textContent).catch(() => {});
      btn.innerHTML = icons.check('', 14);
      setTimeout(() => { btn.innerHTML = icons.copy('', 14); }, 1500);
    });

    // Copy code blocks (delegated)
    responseContent.addEventListener('click', (e) => {
      const copyBtn = e.target.closest('.md-code-copy');
      if (!copyBtn) return;
      const codeEl = copyBtn.closest('.md-code-block')?.querySelector('code');
      if (codeEl) {
        navigator.clipboard.writeText(codeEl.textContent).catch(() => {});
        copyBtn.innerHTML = icons.check('', 14);
        setTimeout(() => { copyBtn.innerHTML = icons.copy('', 14); }, 1500);
      }
    });

    voiceInput.addEventListener('voice-input-submit', async (e) => {
      const { value: content, files } = e.detail;
      if (!content && files.length === 0) return;

      voiceInput.setLoading(true);
      this.classList.add('has-response');
      responseArea.classList.add('is-visible');
      stepsSlot.innerHTML = '';
      const stepsEl = document.createElement('agent-steps');
      stepsSlot.appendChild(stepsEl);
      responseContent.textContent = '';
      responseContent.classList.remove('is-visible');
      responseActions.style.display = 'none';
      responseTrace.style.display = 'none';
      continueLink.style.display = 'none';
      this.querySelector('#response-usage').innerHTML = '';

      try {
        // Create a session so the conversation can be continued
        const sessionRes = await apiFetch('/chat/sessions', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ title: content.slice(0, 100) }),
        });
        const session = sessionRes.ok ? await sessionRes.json() : null;
        const sessionId = session?.session_id;

        // Persist user message
        if (sessionId) {
          apiFetch(`/chat/sessions/${sessionId}/messages`, {
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
            },
            metadata: sessionId ? { session_id: sessionId } : undefined,
          },
        };

        const res = await apiFetch('/orchestrator/a2a', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });
        if (!res.ok) throw new Error(await res.text());

        const { text: reply, traceId, usage } = await this.#readStream(res, stepsEl, responseContent);

        responseActions.style.display = '';
        if (traceId) {
          responseTrace.href = `/session-trace.html?trace_id=${encodeURIComponent(traceId)}`;
          responseTrace.style.display = '';
        }
        this.querySelector('#response-usage').innerHTML = usageChipsHtml(usage);

        // Persist assistant reply (with its usage so chips survive in chat history)
        if (sessionId && reply) {
          const persistBody = { role: 'assistant', content: reply };
          if (traceId || usage) {
            persistBody.usage = {
              input_tokens: usage?.input_tokens ?? null,
              output_tokens: usage?.output_tokens ?? null,
              model: usage?.model ?? null,
              duration_ms: usage?.duration_ms ?? null,
              cost_usd: usage?.cost_usd ?? null,
              estimated: usage?.estimated ?? null,
              trace_id: traceId,
            };
          }
          apiFetch(`/chat/sessions/${sessionId}/messages`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(persistBody),
          }).catch(() => {});
        }

        if (sessionId) {
          continueLink.href = `/chat.html?session_id=${encodeURIComponent(sessionId)}&agent_name=Orchestrator`;
          continueLink.style.display = '';
        }
      } catch (err) {
        stepsEl.finish();
        responseContent.classList.add('is-visible');
        responseContent.innerHTML = `<span style="color:var(--color-error)">Error: ${this.#esc(err.message)}</span>`;
      } finally {
        voiceInput.setLoading(false);
      }
    });
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

  async #readStream(res, stepsEl, responseContent) {
    const typingEl = this.querySelector('#response-typing');
    if (typingEl) typingEl.style.display = '';

    const show = (html, { progress = false } = {}) => {
      if (typingEl) typingEl.style.display = 'none';
      responseContent.classList.add('is-visible');
      responseContent.classList.toggle('is-progress', progress);
      responseContent.innerHTML = html;
    };

    const renderReply = frameRenderer((text) => {
      stepsEl.finish();
      show(renderMarkdown(text));
    });
    const renderProgress = frameRenderer((text) => {
      show(renderMarkdown(text), { progress: true });
    });

    const out = await readA2aStream(res, {
      onReply: renderReply,
      onProgress: renderProgress,
      onData: (d) => stepsEl.onEvent(d),
      onError: (message) => {
        stepsEl.finish();
        show(`<span style="color:var(--color-error)">${this.#esc(message)}</span>`);
      },
    });

    stepsEl.finish();
    if (typingEl) typingEl.style.display = 'none';
    let fullText = out.text;
    if (out.failed && !fullText) {
      fullText = out.errorMessage;
      show(`<span style="color:var(--color-error)">${this.#esc(fullText)}</span>`);
    } else if (!fullText) {
      fullText = 'No response';
      show(this.#esc(fullText));
    } else {
      // Settle on the final text synchronously past any queued frame paint.
      show(renderMarkdown(fullText));
    }
    return { text: fullText, traceId: out.traceId, usage: out.usage };
  }

  #formatName(id) {
    return (id || '').replace(/[-_]/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('orchestrator-page', OrchestratorPage);
