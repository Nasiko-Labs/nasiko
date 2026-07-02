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

const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (orchestrator-page) {
  :scope {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: calc(100dvh - 80px);
    padding: var(--space-md);
    transition: justify-content 0.3s, min-height 0.3s;
  }
  :scope.has-response {
    justify-content: flex-start;
    min-height: auto;
    padding-top: var(--space-md);
  }
  .title {
    font-size: var(--font-size-3xl);
    font-weight: 300;
    color: var(--color-text-main);
    margin-bottom: var(--space-lg);
    text-align: center;
    transition: font-size 0.3s, margin-bottom 0.3s;
  }
  :scope.has-response .title {
    font-size: var(--font-size-lg);
    margin-bottom: var(--space-sm);
  }
  .input-wrap {
    width: min(100%, 680px);
    transition: margin-bottom 0.3s;
  }
  .response-area {
    width: min(100%, 680px);
    margin-top: var(--space-md);
    display: none;
  }
  .response-area.is-visible { display: block; }
  .stream-status {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    padding: var(--space-xs) 0;
    display: flex;
    align-items: center;
    gap: var(--space-xs);
    min-height: 24px;
    transition: opacity 0.3s, min-height 0.3s;
  }
  .stream-status .pulse {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-primary);
    animation: pulse 1.5s ease-in-out infinite;
    flex-shrink: 0;
  }
  @keyframes pulse {
    0%, 100% { opacity: 0.4; transform: scale(0.8); }
    50% { opacity: 1; transform: scale(1.2); }
  }
  .stream-status.is-done { opacity: 0; min-height: 0; padding: 0; overflow: hidden; }
  .status-history {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    margin-bottom: var(--space-xs);
  }
  .status-history-line {
    padding: 2px 0;
    display: flex;
    align-items: center;
    gap: var(--space-xs);
  }
  .status-history-line .dot {
    width: 5px;
    height: 5px;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .dot-thinking { background: var(--color-text-muted); }
  .dot-call { background: var(--color-primary); }
  .dot-result { background: var(--color-success, #22c55e); }
  .dot-rejected { background: var(--color-error, #ef4444); }
  .response-content {
    font-size: var(--font-size-base);
    line-height: 1.6;
    color: var(--color-text-main);
    white-space: pre-wrap;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    padding: var(--space-sm) var(--space-md);
    display: none;
  }
  .response-content.is-visible { display: block; }
  .continue-link {
    display: inline-flex;
    align-items: center;
    gap: var(--space-xs);
    margin-top: var(--space-sm);
    font-size: var(--font-size-xs);
    color: var(--color-primary);
    text-decoration: none;
  }
  .continue-link:hover { text-decoration: underline; }

  .recent-agents {
    width: min(100%, 680px);
    margin-top: var(--space-xl);
  }
  :scope.has-response .recent-agents { display: none; }
  .recent-agents-title {
    font-size: var(--font-size-xs);
    font-weight: 500;
    color: var(--color-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: var(--space-sm);
  }
  .recent-agents-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: var(--space-sm);
  }
  .agent-card {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
    padding: var(--space-sm) var(--space-md);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    text-decoration: none;
    color: var(--color-text-main);
    transition: border-color 0.15s, box-shadow 0.15s;

    &:hover {
      border-color: var(--color-primary);
      box-shadow: 0 0 0 3px var(--color-primary-ring);
    }
  }
  .agent-card-icon {
    width: 28px;
    height: 28px;
    border-radius: var(--radius-sm);
    background: var(--color-primary-subtle);
    color: var(--color-primary);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .agent-card-info {
    min-width: 0;
  }
  .agent-card-name {
    font-size: var(--font-size-sm);
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .agent-card-desc {
    font-size: var(--font-size-xs);
    color: var(--color-text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .agent-card-skel {
    height: 52px;
    border-radius: var(--radius-md);
    background: var(--color-bg-base);
    animation: pulse 1.4s ease-in-out infinite;
  }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class OrchestratorPage extends HTMLElement {
  connectedCallback() {
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
        <div class="recent-agents-title">Recent agents</div>
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

        continueLink.href = `/chat.html?session_id=${sessionId || ''}&agent_name=Orchestrator`;
        continueLink.style.display = '';
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
      const containers = await res.json();
      const recent = (Array.isArray(containers) ? containers : [])
        .filter(c => c.status === 'running')
        .slice(0, 6);

      if (!recent.length) {
        grid.innerHTML = `<span style="font-size:var(--font-size-sm);color:var(--color-text-muted)">No agents running</span>`;
        return;
      }

      grid.innerHTML = recent.map(agent => `
        <a class="agent-card" href="/chat.html?agent_name=${encodeURIComponent(agent.name || agent.id)}">
          <div class="agent-card-icon">${icons.cube('', 14)}</div>
          <div class="agent-card-info">
            <div class="agent-card-name">${this.#esc(agent.name || agent.id)}</div>
            ${agent.image ? `<div class="agent-card-desc">${this.#esc(agent.image.split('/').pop().split(':')[0])}</div>` : ''}
          </div>
        </a>
      `).join('');
    } catch {
      grid.innerHTML = '';
      this.querySelector('#recent-agents')?.remove();
    }
  }

  async #readStream(res, streamStatus, statusHistory, responseContent) {
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
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

          if (evt.statusUpdate) {
            const msg = evt.statusUpdate.status?.message;
            if (msg && msg.parts) {
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

          if (evt.artifactUpdate) {
            const text = evt.artifactUpdate.artifact?.parts
              ?.filter(p => p.text)
              .map(p => p.text)
              .join('');
            if (text) {
              streamStatus.classList.add('is-done');
              responseContent.classList.add('is-visible');
              responseContent.textContent = text;
            }
          }
        } catch {}
      }
    }

    streamStatus.classList.add('is-done');
    if (!responseContent.textContent) {
      responseContent.classList.add('is-visible');
      responseContent.textContent = 'No response';
    }
    return responseContent.textContent;
  }

  #esc(s) {
    const d = document.createElement('span');
    d.textContent = s || '';
    return d.innerHTML;
  }
}

customElements.define('orchestrator-page', OrchestratorPage);
