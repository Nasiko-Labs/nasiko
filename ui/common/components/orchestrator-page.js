import { apiFetch } from '/common/services/api.js';
import { icons } from '/common/utils/icons.js';
import { renderMarkdown } from '/common/utils/markdown.js';
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
      <h1 class="title">What can I help you with?</h1>
      <div class="input-wrap">
        <voice-input
          id="voice-input"
          placeholder="Ask anything — the orchestrator will find the best agent..."
          transcription-callback="transcribeAudio"
        ></voice-input>
      </div>
      <div class="recent-agents" id="recent-agents">
        <div class="recent-agents-title">RECENT AGENTS</div>
        <div class="recent-agents-grid" id="recent-agents-grid">
          <div class="agent-card-skel"></div>
          <div class="agent-card-skel"></div>
          <div class="agent-card-skel"></div>
        </div>
      </div>
      <div class="response-area" id="response-area">
        <div class="steps-slot" id="steps-slot"></div>
        <div class="response-wrap">
          <div class="response-content md-body" id="response-content"></div>
          <div class="msg-actions" id="response-actions" style="display:none;">
            <button type="button" class="msg-action-copy" aria-label="Copy response" title="Copy">${icons.copy('', 14)}</button>
            <a class="msg-action-trace" id="response-trace" style="display:none;" href="#" aria-label="View trace" title="View trace">${icons.trace('', 14)}</a>
          </div>
        </div>
        <a class="continue-link" id="continue-link" style="display:none;" href="#">Continue in chat</a>
      </div>
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

      try {
        // Create a session so the conversation can be continued
        const sessionRes = await apiFetch('/chat/sessions', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ title: content.slice(0, 100) }),
        });
        const session = sessionRes.ok ? await sessionRes.json() : null;
        const sessionId = session?.session_id || session?.id;

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

        const { text: reply, traceId } = await this.#readStream(res, stepsEl, responseContent);

        responseActions.style.display = '';
        if (traceId) {
          responseTrace.href = `/session-trace.html?trace_id=${encodeURIComponent(traceId)}`;
          responseTrace.style.display = '';
        }

        // Persist assistant reply
        if (sessionId && reply) {
          apiFetch(`/chat/sessions/${sessionId}/messages`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ role: 'assistant', content: reply, ...(traceId && { trace_id: traceId }) }),
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
            <div class="agent-card-icon">${icons.cube('', 14)}</div>
            <div class="agent-card-info">
              <div class="agent-card-name">${this.#esc(displayName)}</div>
            </div>
          </a>
        `;
      }).join('');
    } catch {
      grid.innerHTML = '';
      this.querySelector('#recent-agents')?.remove();
    }
  }

  async #readStream(res, stepsEl, responseContent) {
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    let fullText = '';
    let traceId = null;

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop();

      for (const line of lines) {
        if (!line.startsWith('data: ')) continue;
        const raw = line.slice(6).trim();
        if (!raw) continue;
        try {
          const evt = JSON.parse(raw);
          const statusUpdate = evt.statusUpdate || evt.result?.statusUpdate;
          const artifactUpdate = evt.artifactUpdate || evt.result?.artifactUpdate;

          if (statusUpdate) {
            const state = statusUpdate.status?.state;
            const msg = statusUpdate.status?.message;
            if (msg && msg.parts) {
              if (state === 'TASK_STATE_COMPLETED') {
                const text = msg.parts.filter(p => p.text).map(p => p.text).join('');
                if (text && !fullText) {
                  fullText = text;
                  stepsEl.finish();
                  responseContent.classList.add('is-visible');
                  responseContent.innerHTML = renderMarkdown(fullText);
                }
              } else if (state === 'TASK_STATE_FAILED') {
                const text = msg.parts.filter(p => p.text).map(p => p.text).join('');
                if (text) {
                  stepsEl.finish();
                  responseContent.classList.add('is-visible');
                  responseContent.innerHTML = `<span style="color:var(--color-error)">${this.#esc(text)}</span>`;
                }
              }
              for (const part of msg.parts) {
                if (!part.data) continue;
                if (part.data.type === 'trace_meta' && part.data.trace_id) {
                  traceId = part.data.trace_id;
                  continue;
                }
                stepsEl.onEvent(part.data);
              }
            }
          }

          if (artifactUpdate) {
            const au = artifactUpdate;
            const text = au.artifact?.parts
              ?.filter(p => p.text)
              .map(p => p.text)
              .join('');
            if (text) {
              if (au.append) {
                fullText += text;
              } else {
                fullText = text;
              }
              stepsEl.finish();
              responseContent.classList.add('is-visible');
              responseContent.innerHTML = renderMarkdown(fullText);
            }
          }
        } catch {}
      }
    }

    stepsEl.finish();
    if (!fullText) {
      fullText = 'No response';
      responseContent.classList.add('is-visible');
      responseContent.textContent = fullText;
    } else {
      responseContent.innerHTML = renderMarkdown(fullText);
    }
    return { text: fullText, traceId };
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
