import { apiFetch } from '/common/services/api.js';
import "./voice-input.js";
import { icons } from '/common/utils/icons.js';
import { renderMarkdown } from '/common/utils/markdown.js';

if (!window.transcribeAudio) {
  window.transcribeAudio = async (blob) => {
    const form = new FormData();
    form.append('file', blob, 'audio.webm');
    const res = await apiFetch('/transcribe', { method: 'POST', body: form });
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    return data.text;
  };
}

import styles from './chat-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class ChatPage extends HTMLElement {
  #initialized = false;
  #sessionId = null;
  #contextId = null;
  #agentId = null;
  #agentLabel = null;
  #lastUserContent = null;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    const params = new URLSearchParams(location.search);
    this.#agentId = params.get("agent_id");
    this.#sessionId = params.get("session_id") || null;
    this.#contextId = params.get("context_id");
    this.#agentLabel = params.get("agent_name") || "Agent";

    if (this.#agentId) document.title = `Nasiko — Chat with ${this.#agentLabel}`;

    this.#render();
    this.#bindEvents();

    if (this.#sessionId) {
      const messagesEl = this.querySelector("#messages");
      messagesEl.innerHTML = `
        <div class="msg-skel"><div class="msg-skel-line" style="width:60%"></div></div>
        <div class="msg-skel is-right"><div class="msg-skel-line" style="width:45%"></div></div>
        <div class="msg-skel"><div class="msg-skel-line" style="width:70%"></div></div>
      `;
      this.#loadMessages(messagesEl);
    }
  }

  #render() {
    const initial = this.#agentLabel.charAt(0).toUpperCase();
    const agentCardUrl = this.#agentId ? `/agent-card.html?id=${encodeURIComponent(this.#agentId)}` : null;

    this.innerHTML = `
      <div class="chat-header">
        <div class="chat-header-avatar" aria-hidden="true">${initial}</div>
        <div class="chat-header-info">
          <span class="chat-agent-name">${this.#esc(this.#agentLabel)}</span>
          <span class="chat-agent-status"><span class="status-dot"></span> Running</span>
        </div>
        ${agentCardUrl ? `<a class="chat-header-link" href="${agentCardUrl}" title="View agent card">${icons.externalLink('', 16)}</a>` : ''}
      </div>
      <div class="messages" id="messages">
        ${this.#sessionId ? '' : this.#renderWelcome()}
      </div>
      <div class="input-area">
        <voice-input
          id="chat-input"
          placeholder="Type a message..."
          transcription-callback="transcribeAudio"
        ></voice-input>
      </div>
    `;
  }

  #renderWelcome() {
    const prompts = [
      "Help me debug a failing deployment",
      "Explain how container networking works",
      "Generate a Dockerfile for my service",
    ];
    return `
      <div class="welcome-state">
        <div class="welcome-avatar" aria-hidden="true">${this.#agentLabel.charAt(0).toUpperCase()}</div>
        <h2 class="welcome-title">${this.#esc(this.#agentLabel)}</h2>
        <p class="welcome-subtitle">Ask me anything</p>
        <div class="welcome-prompts">
          ${prompts.map(p => `<button type="button" class="welcome-chip">${this.#esc(p)}</button>`).join('')}
        </div>
      </div>
    `;
  }

  #bindEvents() {
    const messagesEl = this.querySelector("#messages");
    const chatInput = this.querySelector("#chat-input");

    // Welcome prompt chips
    const chips = this.querySelectorAll(".welcome-chip");
    for (const chip of chips) {
      chip.addEventListener("click", () => {
        const textarea = chatInput.querySelector('#textarea');
        if (textarea) {
          textarea.value = chip.textContent;
          textarea.focus();
        }
      });
    }

    // Copy code blocks (delegated)
    messagesEl.addEventListener("click", (e) => {
      const copyBtn = e.target.closest(".md-code-copy");
      if (copyBtn) {
        const codeEl = copyBtn.closest(".md-code-block")?.querySelector("code");
        if (codeEl) {
          navigator.clipboard.writeText(codeEl.textContent).catch(() => {});
          copyBtn.innerHTML = icons.check('', 14);
          setTimeout(() => { copyBtn.innerHTML = icons.copy('', 14); }, 1500);
        }
        return;
      }

      // Message action: copy
      const msgCopyBtn = e.target.closest(".msg-action-copy");
      if (msgCopyBtn) {
        const row = msgCopyBtn.closest(".msg-row");
        const msgEl = row?.querySelector(".msg, .stream-content");
        if (msgEl) {
          navigator.clipboard.writeText(msgEl.textContent).catch(() => {});
          msgCopyBtn.innerHTML = icons.check('', 14);
          setTimeout(() => { msgCopyBtn.innerHTML = icons.copy('', 14); }, 1500);
        }
        return;
      }

      // Message action: retry
      const retryBtn = e.target.closest(".msg-action-retry");
      if (retryBtn && this.#lastUserContent) {
        this.#sendMessage(this.#lastUserContent);
      }
    });

    chatInput.addEventListener("voice-input-submit", async (e) => {
      const content = e.detail.value;
      if (!content) {
        chatInput.setLoading(false);
        return;
      }
      this.#sendMessage(content);
    });
  }

  async #sendMessage(content) {
    const messagesEl = this.querySelector("#messages");
    const chatInput = this.querySelector("#chat-input");

    // Remove welcome state if present
    const welcome = this.querySelector(".welcome-state");
    if (welcome) welcome.remove();

    this.#lastUserContent = content;
    this.#appendMsg(messagesEl, "user", content);
    chatInput.reset();
    chatInput.setLoading(true);

    try {
      if (!this.#sessionId) {
        const res = await apiFetch("/chat/sessions", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ agent_id: this.#agentId }),
        });
        if (!res.ok) throw new Error("Failed to create session");
        const session = await res.json();
        this.#sessionId = session.session_id || session.id;
        const params = new URLSearchParams(location.search);
        const nameParam = params.get("agent_name")
          ? `&agent_name=${encodeURIComponent(params.get("agent_name"))}`
          : "";
        history.replaceState(
          null,
          "",
          `/chat.html?agent_id=${this.#agentId}&session_id=${this.#sessionId}${nameParam}`,
        );
      }

      if (!this.#contextId) {
        this.#contextId = crypto.randomUUID
          ? crypto.randomUUID()
          : Math.random().toString(36).slice(2) + Date.now().toString(36);
      }

      this.#persistMessage(this.#sessionId, "user", content);

      const body = {
        jsonrpc: "2.0",
        id: crypto.randomUUID
          ? crypto.randomUUID()
          : Math.random().toString(36).slice(2) + Date.now().toString(36),
        method: "message/stream",
        params: {
          message: {
            messageId: crypto.randomUUID
              ? crypto.randomUUID()
              : Math.random().toString(36).slice(2) + Date.now().toString(36),
            contextId: this.#contextId,
            agentId: this.#agentId || undefined,
            role: "ROLE_USER",
            parts: [{ text: content }],
          },
          metadata: {
            ...(this.#agentId && { agent_id: this.#agentId }),
            ...(this.#sessionId && { session_id: this.#sessionId }),
          },
        },
      };

      const res = await apiFetch("/orchestrator/a2a", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!res.ok) {
        const errBody = await res.text();
        try {
          const j = JSON.parse(errBody);
          throw new Error(j.error?.message || errBody);
        } catch (parseErr) {
          if (parseErr.message !== errBody) throw parseErr;
          throw new Error(errBody);
        }
      }

      const { text: reply, traceId } = await this.#readA2aStream(res, messagesEl);
      this.#persistMessage(this.#sessionId, "assistant", reply, traceId);
      if (traceId) {
        const lastRow = messagesEl.querySelector(".msg-row.is-assistant:last-child .assistant-stream");
        if (lastRow) {
          const traceBtn = document.createElement("a");
          traceBtn.className = "msg-trace-btn";
          traceBtn.href = `/session-trace.html?trace_id=${traceId}`;
          traceBtn.title = "View trace";
          traceBtn.innerHTML = icons.trace('', 14);
          lastRow.appendChild(traceBtn);
        }
      }
      this.#updateRetryButtons(messagesEl);
    } catch (err) {
      this.#appendMsg(messagesEl, "assistant", `Error: ${err.message}`);
      this.#updateRetryButtons(messagesEl);
    } finally {
      chatInput.setLoading(false);
    }
  }

  async #loadMessages(messagesEl) {
    try {
      const res = await apiFetch(`/chat/sessions/${this.#sessionId}/messages`);
      if (!res.ok) {
        messagesEl.innerHTML = '';
        return;
      }
      const result = await res.json();
      const msgs = result.data || result;
      messagesEl.innerHTML = '';
      if (Array.isArray(msgs) && msgs.length) {
        for (const m of msgs) {
          this.#appendMsg(messagesEl, m.role, m.content, m.trace_id);
          if (m.role === 'user') this.#lastUserContent = m.content;
        }
        this.#updateRetryButtons(messagesEl);
      }
    } catch { messagesEl.innerHTML = ''; }
  }

  #appendMsg(messagesEl, role, content, traceId) {
    const row = document.createElement("div");
    row.className = `msg-row is-${role}`;

    const div = document.createElement("div");
    div.className = `msg is-${role}${role === 'assistant' ? ' md-body' : ''}`;

    if (role === 'assistant') {
      div.innerHTML = renderMarkdown(content);
    } else {
      div.textContent = content;
    }

    row.appendChild(div);

    // Message actions toolbar
    if (role === 'assistant') {
      const actions = document.createElement("div");
      actions.className = "msg-actions";
      actions.innerHTML = `
        <button type="button" class="msg-action-copy" aria-label="Copy message" title="Copy">${icons.copy('', 14)}</button>
      `;
      row.appendChild(actions);
    }

    if (traceId) {
      const traceBtn = document.createElement("a");
      traceBtn.className = "msg-trace-btn";
      traceBtn.href = `/session-trace.html?trace_id=${traceId}`;
      traceBtn.title = "View trace";
      traceBtn.innerHTML = icons.trace('', 14);
      row.appendChild(traceBtn);
    }

    messagesEl.appendChild(row);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  #updateRetryButtons(messagesEl) {
    // Remove existing retry buttons
    for (const btn of messagesEl.querySelectorAll(".msg-action-retry")) {
      btn.remove();
    }
    // Add retry only to the last assistant message
    const lastAssistant = messagesEl.querySelector(".msg-row.is-assistant:last-child .msg-actions");
    if (lastAssistant && this.#lastUserContent) {
      const retryBtn = document.createElement("button");
      retryBtn.type = "button";
      retryBtn.className = "msg-action-retry";
      retryBtn.setAttribute("aria-label", "Retry");
      retryBtn.title = "Retry";
      retryBtn.innerHTML = icons.refresh('', 14);
      lastAssistant.appendChild(retryBtn);
    }
  }

  async #readA2aStream(res, messagesEl) {
    // Create unified streaming area
    const streamRow = document.createElement("div");
    streamRow.className = "msg-row is-assistant";
    const streamArea = document.createElement("div");
    streamArea.className = "assistant-stream";

    const statusEl = document.createElement("div");
    statusEl.className = "stream-status";
    statusEl.innerHTML = `<span class="pulse"></span> ...`;

    const contentEl = document.createElement("div");
    contentEl.className = "stream-content md-body";

    streamArea.appendChild(statusEl);
    streamArea.appendChild(contentEl);
    streamRow.appendChild(streamArea);
    messagesEl.appendChild(streamRow);
    messagesEl.scrollTop = messagesEl.scrollHeight;

    let fullText = "";
    let traceId = null;
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split("\n");
      buffer = lines.pop();

      for (const line of lines) {
        if (!line.startsWith("data: ")) continue;
        const raw = line.slice(6).trim();
        if (!raw) continue;
        try {
          const evt = JSON.parse(raw);
          const statusUpdate = evt.statusUpdate || evt.result?.statusUpdate;
          const artifactUpdate = evt.artifactUpdate || evt.result?.artifactUpdate;

          if (statusUpdate) {
            const su = statusUpdate;
            const state = su.status?.state;
            const msg = su.status?.message;
            if (msg && msg.parts) {
              if (state === "TASK_STATE_COMPLETED") {
                const text = msg.parts.filter(p => p.text).map(p => p.text).join("");
                if (text && !fullText) {
                  fullText = text;
                  statusEl.classList.add("is-done");
                  contentEl.classList.add("is-visible");
                  contentEl.innerHTML = renderMarkdown(fullText);
                  messagesEl.scrollTop = messagesEl.scrollHeight;
                }
              } else if (state === "TASK_STATE_FAILED") {
                const text = msg.parts.filter(p => p.text).map(p => p.text).join("");
                if (text) {
                  statusEl.classList.add("is-done");
                  contentEl.classList.add("is-visible");
                  contentEl.innerHTML = `<span style="color:var(--color-error)">${this.#esc(text)}</span>`;
                  messagesEl.scrollTop = messagesEl.scrollHeight;
                }
              }
              for (const part of msg.parts) {
                if (part.data) {
                  const d = part.data;
                  if (d.type === "trace_meta" && d.trace_id) {
                    traceId = d.trace_id;
                    continue;
                  }
                  let text = "";
                  if (d.type === "thinking") {
                    text = d.content || "Thinking...";
                  } else if (d.type === "tool_call") {
                    text = `Calling ${d.agent}...`;
                  } else if (d.type === "tool_result") {
                    text = `${d.agent} responded`;
                  } else if (d.type === "agent_invoke") {
                    text = `${d.caller_agent} calling ${d.target_agent}...`;
                  } else if (d.type === "agent_result") {
                    text = `${d.target_agent} responded`;
                  } else if (d.type === "policy_rejected") {
                    text = `${d.agent} blocked: ${d.reason}`;
                  }
                  if (text) {
                    statusEl.innerHTML = `<span class="pulse"></span> ${this.#esc(text)}`;
                    messagesEl.scrollTop = messagesEl.scrollHeight;
                  }
                }
              }
            }
          }

          if (artifactUpdate) {
            const au = artifactUpdate;
            const text = au.artifact?.parts
              ?.filter((p) => p.text)
              .map((p) => p.text)
              .join("");
            if (text) {
              if (au.append) {
                fullText += text;
              } else {
                fullText = text;
              }
              statusEl.classList.add("is-done");
              contentEl.classList.add("is-visible");
              contentEl.innerHTML = renderMarkdown(fullText);
              messagesEl.scrollTop = messagesEl.scrollHeight;
            }
          }
        } catch {}
      }
    }

    // Finalize
    statusEl.classList.add("is-done");
    if (!fullText) {
      contentEl.classList.add("is-visible");
      contentEl.innerHTML = renderMarkdown("No response");
      fullText = "No response";
    }

    // Add actions to stream row
    const actions = document.createElement("div");
    actions.className = "msg-actions";
    actions.innerHTML = `
      <button type="button" class="msg-action-copy" aria-label="Copy message" title="Copy">${icons.copy('', 14)}</button>
    `;
    streamArea.appendChild(actions);

    return { text: fullText, traceId };
  }

  #persistMessage(sessionId, role, content, traceId) {
    const payload = { role, content };
    if (traceId) payload.trace_id = traceId;
    apiFetch(`/chat/sessions/${sessionId}/messages`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    }).catch(() => {});
  }

  #esc(s) {
    const d = document.createElement("span");
    d.textContent = s || "";
    return d.innerHTML;
  }
}

customElements.define("chat-page", ChatPage);
