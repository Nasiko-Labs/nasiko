import "./voice-input.js";

import styles from './chat-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class ChatPage extends HTMLElement {
  #sessionId = null;
  #contextId = null;

  connectedCallback() {
    const params = new URLSearchParams(location.search);
    const agentId = params.get("agent_id");
    this.#sessionId = params.get("session_id");
    this.#contextId = params.get("context_id");

    const agentLabel = params.get("agent_name") || "Agent";

    if (agentId) document.title = `Nasiko — Chat with ${agentLabel}`;

    this.innerHTML = `
      <div class="chat-header">
        <span class="chat-agent-name">${agentLabel}</span>
${agentId ? `<app-badge variant="info">${agentId}</app-badge>` : ""}
      </div>
      <div class="messages" id="messages"></div>
      <div class="input-area">
        <voice-input
          id="chat-input"
          placeholder="Type a message..."
          transcription-callback="transcribeAudio"
        ></voice-input>
      </div>
    `;

    const messagesEl = this.querySelector("#messages");
    const chatInput = this.querySelector("#chat-input");

    if (this.#sessionId) this.#loadMessages(messagesEl);

    chatInput.addEventListener("voice-input-submit", async (e) => {
      const content = e.detail.value;
      if (!content) {
        chatInput.setLoading(false);
        return;
      }

      this.#appendMsg(messagesEl, "user", content);
      chatInput.reset();
      chatInput.setLoading(true);

      try {
        if (!this.#sessionId) {
          const res = await fetch("/api/chat/sessions", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ agent_id: agentId }),
          });
          if (!res.ok) throw new Error("Failed to create session");
          const session = await res.json();
          this.#sessionId = session.session_id || session.id;
          const nameParam = params.get("agent_name")
            ? `&agent_name=${encodeURIComponent(params.get("agent_name"))}`
            : "";
          history.replaceState(
            null,
            "",
            `/chat.html?agent_id=${agentId}&session_id=${this.#sessionId}${nameParam}`,
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
              agentId: agentId || undefined,
              role: "ROLE_USER",
              parts: [{ text: content }],
            },
            metadata: {
              ...(agentId && { agent_id: agentId }),
              ...(this.#sessionId && { session_id: this.#sessionId }),
            },
          },
        };

        const res = await fetch("/api/a2a", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        });
        if (!res.ok) {
          const errBody = await res.text();
          try {
            const j = JSON.parse(errBody);
            throw new Error(j.error?.message || errBody);
          } catch (e) {
            if (e.message !== errBody) throw e;
            throw new Error(errBody);
          }
        }

        const reply = await this.#readA2aStream(res, messagesEl);
        this.#persistMessage(this.#sessionId, "assistant", reply);
      } catch (err) {
        this.#appendMsg(messagesEl, "assistant", `Error: ${err.message}`);
      } finally {
        chatInput.setLoading(false);
      }
    });
  }

  async #loadMessages(messagesEl) {
    try {
      const res = await fetch(`/api/chat/sessions/${this.#sessionId}/messages`);
      if (!res.ok) return;
      const result = await res.json();
      const msgs = result.data || result;
      if (Array.isArray(msgs)) {
        msgs.forEach((m) => this.#appendMsg(messagesEl, m.role, m.content));
      }
    } catch {}
  }

  #appendMsg(messagesEl, role, content) {
    const row = document.createElement("div");
    row.className = `msg-row is-${role}`;
    const div = document.createElement("div");
    div.className = `msg is-${role}`;
    div.textContent = content;
    row.appendChild(div);
    messagesEl.appendChild(row);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  async #readA2aStream(res, messagesEl) {
    // Create unified streaming area
    const streamRow = document.createElement("div");
    streamRow.className = "msg-row is-assistant";
    const streamArea = document.createElement("div");
    streamArea.className = "assistant-stream";

    const statusEl = document.createElement("div");
    statusEl.className = "stream-status";
    statusEl.innerHTML = '<span class="pulse"></span> ...';

    const contentEl = document.createElement("div");
    contentEl.className = "stream-content";

    streamArea.appendChild(statusEl);
    streamArea.appendChild(contentEl);
    streamRow.appendChild(streamArea);
    messagesEl.appendChild(streamRow);
    messagesEl.scrollTop = messagesEl.scrollHeight;

    let fullText = "";
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

          if (evt.statusUpdate) {
            const su = evt.statusUpdate;
            const msg = su.status?.message;
            if (msg && msg.parts) {
              for (const part of msg.parts) {
                if (part.data) {
                  const d = part.data;
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

          if (evt.artifactUpdate) {
            const au = evt.artifactUpdate;
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
              contentEl.textContent = fullText;
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
      contentEl.textContent = "No response";
      fullText = "No response";
    }

    return fullText;
  }

  #persistMessage(sessionId, role, content) {
    fetch(`/api/chat/sessions/${sessionId}/messages`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ role, content }),
    }).catch(() => {});
  }

  #esc(s) {
    const d = document.createElement("span");
    d.textContent = s || "";
    return d.innerHTML;
  }
}

customElements.define("chat-page", ChatPage);
