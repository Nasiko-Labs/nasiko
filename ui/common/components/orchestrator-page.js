import { icons } from '/common/utils/icons.js';
import '/common/components/voice-input.js';

window.transcribeAudio = async (blob) => {
  const form = new FormData();
  form.append('file', blob, 'audio.webm');
  const res = await fetch('/api/transcribe', { method: 'POST', body: form });
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
        <div class="status-history" id="status-history"></div>
        <div class="stream-status" id="stream-status"><span class="pulse"></span> ...</div>
        <div class="response-content" id="response-content"></div>
        <a class="continue-link" id="continue-link" style="display:none;" href="#">Continue in chat</a>
      </div>
    `;

    this.#loadRecentAgents();

    const voiceInput = this.querySelector('#voice-input');
    const responseArea = this.querySelector('#response-area');
    const statusHistory = this.querySelector('#status-history');
    const streamStatus = this.querySelector('#stream-status');
    const responseContent = this.querySelector('#response-content');
    const continueLink = this.querySelector('#continue-link');
    continueLink.insertAdjacentHTML('beforeend', ' ' + icons.chevronRight('', 14));

    voiceInput.addEventListener('voice-input-submit', async (e) => {
      const { value: content, files } = e.detail;
      if (!content && files.length === 0) return;

      voiceInput.setLoading(true);
      this.classList.add('has-response');
      responseArea.classList.add('is-visible');
      statusHistory.innerHTML = '';
      streamStatus.innerHTML = '<span class="pulse"></span> ...';
      streamStatus.classList.remove('is-done');
      responseContent.textContent = '';
      responseContent.classList.remove('is-visible');
      continueLink.style.display = 'none';

      try {
        // Create a session so the conversation can be continued
        const sessionRes = await fetch('/api/chat/sessions', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ title: content.slice(0, 100) }),
        });
        const session = sessionRes.ok ? await sessionRes.json() : null;
        const sessionId = session?.session_id || session?.id;

        // Persist user message
        if (sessionId) {
          fetch(`/api/chat/sessions/${sessionId}/messages`, {
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

        const res = await fetch('/api/a2a', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        });
        if (!res.ok) throw new Error(await res.text());

        const reply = await this.#readStream(res, streamStatus, statusHistory, responseContent);

        // Persist assistant reply
        if (sessionId && reply) {
          fetch(`/api/chat/sessions/${sessionId}/messages`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ role: 'assistant', content: reply }),
          }).catch(() => {});
        }

        if (sessionId) {
          continueLink.href = `/chat.html?session_id=${encodeURIComponent(sessionId)}&agent_name=Orchestrator`;
          continueLink.style.display = '';
        }
      } catch (err) {
        streamStatus.classList.add('is-done');
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
      const res = await fetch('/api/containers');
      if (!res.ok) throw new Error('Failed to fetch');
      const body = await res.json();
      const containers = Array.isArray(body) ? body : (body.data || []);
      const recent = containers
        .filter(c => (c.state || c.status) === 'running')
        .slice(0, 6);

      if (!recent.length) {
        grid.innerHTML = `<span style="font-size:var(--font-size-sm);color:var(--color-text-muted)">No agents running</span>`;
        return;
      }

      grid.innerHTML = recent.map(agent => {
        const id = agent.container_id || agent.name || agent.id;
        const displayName = agent.display_name || this.#formatName(id);
        return `
          <a class="agent-card" href="/chat.html?agent_name=${encodeURIComponent(displayName)}&agent_id=${encodeURIComponent(id)}">
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

  async #readStream(res, streamStatus, statusHistory, responseContent) {
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    let fullText = '';
    let lastStatus = '';
    let lastDotClass = '';

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
                  streamStatus.classList.add('is-done');
                  responseContent.classList.add('is-visible');
                  responseContent.textContent = fullText;
                }
              } else if (state === 'TASK_STATE_FAILED') {
                const text = msg.parts.filter(p => p.text).map(p => p.text).join('');
                if (text) {
                  streamStatus.classList.add('is-done');
                  responseContent.classList.add('is-visible');
                  responseContent.innerHTML = `<span style="color:var(--color-error)">${this.#esc(text)}</span>`;
                }
              }
              for (const part of msg.parts) {
                if (!part.data) continue;
                const d = part.data;
                let text = '';
                let dotClass = 'dot-thinking';
                if (d.type === 'thinking') {
                  text = d.content || 'Thinking...';
                  dotClass = 'dot-thinking';
                } else if (d.type === 'tool_call') {
                  text = `Calling <strong>${this.#esc(d.agent)}</strong>`;
                  dotClass = 'dot-call';
                } else if (d.type === 'tool_result') {
                  text = `<strong>${this.#esc(d.agent)}</strong> responded`;
                  dotClass = 'dot-result';
                } else if (d.type === 'agent_invoke') {
                  text = `<strong>${this.#esc(d.caller_agent)}</strong> calling <strong>${this.#esc(d.target_agent)}</strong>`;
                  dotClass = 'dot-call';
                } else if (d.type === 'agent_result') {
                  text = `<strong>${this.#esc(d.target_agent)}</strong> responded`;
                  dotClass = 'dot-result';
                } else if (d.type === 'policy_rejected') {
                  text = `<strong>${this.#esc(d.agent)}</strong> blocked: ${this.#esc(d.reason)}`;
                  dotClass = 'dot-rejected';
                }

                if (text && text !== lastStatus) {
                  if (lastStatus && lastDotClass) {
                    const histLine = document.createElement('div');
                    histLine.className = 'status-history-line';
                    histLine.innerHTML = `<span class="dot ${lastDotClass}"></span> ${lastStatus}`;
                    statusHistory.appendChild(histLine);
                    while (statusHistory.children.length > 3) {
                      statusHistory.removeChild(statusHistory.firstChild);
                    }
                  }
                  lastStatus = text;
                  lastDotClass = dotClass;
                  streamStatus.innerHTML = `<span class="pulse"></span> ${text}`;
                }
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
              streamStatus.classList.add('is-done');
              responseContent.classList.add('is-visible');
              responseContent.textContent = fullText;
            }
          }
        } catch {}
      }
    }

    streamStatus.classList.add('is-done');
    if (!fullText) {
      fullText = 'No response';
      responseContent.classList.add('is-visible');
      responseContent.textContent = fullText;
    }
    return fullText;
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
