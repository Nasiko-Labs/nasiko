import { apiFetch } from '/common/services/api.js';
import "./voice-input.js";
import "./agent-steps.js";
import "./app-module-nav.js";
import { icons } from '/common/utils/icons.js';
import { renderMarkdown } from '/common/utils/markdown.js';
import { readA2aStream, frameRenderer, nearBottom } from '/common/utils/a2a-stream.js';
import { usageChipsHtml, usageFromMessage } from '/common/utils/usage-chips.js';
import { transcribeBlob } from '/common/utils/voice-utils.js';

if (!window.transcribeAudio) {
  window.transcribeAudio = transcribeBlob;
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
  #sampleQueries = [];
  #sending = false;

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
        <div class="msg-skel is-right"><div class="msg-skel-bubble" style="width:38%"></div></div>
        <div class="msg-skel"><div class="msg-skel-block" style="width:78%">
          <div class="msg-skel-line" style="width:96%"></div>
          <div class="msg-skel-line" style="width:88%"></div>
          <div class="msg-skel-line" style="width:61%"></div>
        </div></div>
        <div class="msg-skel is-right"><div class="msg-skel-bubble" style="width:24%"></div></div>
        <div class="msg-skel"><div class="msg-skel-block" style="width:70%">
          <div class="msg-skel-line" style="width:92%"></div>
          <div class="msg-skel-line" style="width:44%"></div>
        </div></div>
      `;
      this.#loadMessages(messagesEl);
    } else if (this.#agentId) {
      this.#loadSampleQueries();
    }
  }

  #render() {
    const initial = this.#agentLabel.charAt(0).toUpperCase();
    const agentCardUrl = this.#agentId ? `/agent-card.html?id=${encodeURIComponent(this.#agentId)}` : null;

    this.innerHTML = `
      ${this.#agentId ? '' : '<app-module-nav module="orchestrator"></app-module-nav>'}
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

  #renderWelcome(prompts) {
    const chips = (prompts || []).length
      ? prompts
      : ["Help me debug a failing deployment", "Explain how container networking works", "Generate a Dockerfile for my service"];
    return `
      <div class="welcome-state">
        <div class="welcome-avatar" aria-hidden="true">${this.#agentLabel.charAt(0).toUpperCase()}</div>
        <h2 class="welcome-title">${this.#esc(this.#agentLabel)}</h2>
        <p class="welcome-subtitle">Ask me anything</p>
        <div class="welcome-prompts">
          ${chips.map(p => `<button type="button" class="welcome-chip">${this.#esc(p)}</button>`).join('')}
        </div>
      </div>
    `;
  }

  async #loadSampleQueries() {
    try {
      const res = await apiFetch(`/agents/${encodeURIComponent(this.#agentId)}`);
      if (!res.ok) { console.warn('loadSampleQueries: fetch failed', res.status); return; }
      const body = await res.json();
      const agent = body.data || body;
      if (agent.display_name) {
        this.#agentLabel = agent.display_name;
      }
      const skills = agent.skills || [];
      const queries = skills
        .map(s => s.sample_query || (Array.isArray(s.examples) && s.examples[0]) || null)
        .filter(Boolean)
        .slice(0, 3);
      if (!queries.length) { console.warn('loadSampleQueries: no examples found in skills', skills); return; }
      this.#sampleQueries = queries;
      const welcome = this.querySelector('.welcome-state');
      if (!welcome) { console.warn('loadSampleQueries: .welcome-state not found in DOM'); return; }
      welcome.outerHTML = this.#renderWelcome(queries);
      this.#bindWelcomeChips();
    } catch (err) { console.warn('loadSampleQueries failed:', err); }
  }

  #bindWelcomeChips() {
    const chatInput = this.querySelector("#chat-input");
    for (const chip of this.querySelectorAll(".welcome-chip")) {
      chip.addEventListener("click", () => {
        const textarea = chatInput.querySelector('#textarea');
        if (textarea) {
          textarea.value = chip.textContent;
          textarea.focus();
        }
      });
    }
  }

  #bindEvents() {
    const messagesEl = this.querySelector("#messages");
    const chatInput = this.querySelector("#chat-input");

    // Welcome prompt chips
    this.#bindWelcomeChips();

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
    if (this.#sending) return;
    this.#sending = true;
    const messagesEl = this.querySelector("#messages");
    const chatInput = this.querySelector("#chat-input");

    // Remove welcome state if present
    const welcome = this.querySelector(".welcome-state");
    if (welcome) welcome.remove();

    this.#lastUserContent = content;
    this.#appendMsg(messagesEl, "user", content);
    chatInput.reset();
    chatInput.setLoading(true);

    // Immediate feedback: typing dots from the moment the prompt is sent —
    // session create + response headers can take seconds on slow agents.
    const pendingRow = document.createElement("div");
    pendingRow.className = "msg-row is-assistant";
    pendingRow.innerHTML = `<div class="typing-indicator" aria-label="Agent is responding"><span></span><span></span><span></span></div>`;
    messagesEl.appendChild(pendingRow);
    messagesEl.scrollTop = messagesEl.scrollHeight;

    try {
      if (!this.#sessionId) {
        const res = await apiFetch("/chat/sessions", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ agent_id: this.#agentId }),
        });
        if (!res.ok) throw new Error("Failed to create session");
        const body = await res.json();
        // POST /api/chat/sessions wraps the session in {data, status_code,
        // message}; tolerate a bare object for older servers.
        const session = body.data || body;
        this.#sessionId = session.session_id || session.id;
        if (!this.#sessionId) throw new Error("Session created without an id");
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

      // Reuse the CP session id as the A2A contextId, the same convention the
      // CLI follows (see a2a_dispatch.rs "the CLI reuses its CP session id as
      // contextId"). A freshly minted random id here broke observability: the
      // server keys `session_traces` and the dispatch span's `session.id` on
      // contextId, so traces landed under a throwaway id while every link in
      // the UI (and this page's URL) carries `session_id` — the session detail
      // page then 404'd and showed no traces at all. It also cost multi-turn
      // continuity, since a reloaded session got a brand-new contextId.
      if (!this.#contextId) this.#contextId = this.#sessionId;

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

      pendingRow.remove();
      const { text: reply, traceId, usage } = await this.#readA2aStream(res, messagesEl);
      this.#persistMessage(this.#sessionId, "assistant", reply, { traceId, usage });
      this.#updateRetryButtons(messagesEl);
    } catch (err) {
      pendingRow.remove();
      this.#appendMsg(messagesEl, "assistant", `Error: ${err.message}`);
      this.#updateRetryButtons(messagesEl);
    } finally {
      this.#sending = false;
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
          this.#appendMsg(messagesEl, m.role, m.content, {
            usage: usageFromMessage(m),
            traceId: m.trace_id,
          });
          if (m.role === 'user') this.#lastUserContent = m.content;
        }
        this.#updateRetryButtons(messagesEl);
      }
    } catch { messagesEl.innerHTML = ''; }
  }

  #appendMsg(messagesEl, role, content, { usage = null, traceId = null } = {}) {
    // Sessions are written by multiple clients: the web UI stores replies as
    // "assistant" while the CLI/TUI store them as "agent". Anything that is
    // not the user renders as an agent reply (markdown + assistant styling).
    const isUser = role === 'user';
    const roleClass = isUser ? 'is-user' : 'is-assistant';

    const row = document.createElement("div");
    row.className = `msg-row ${roleClass}`;

    const div = document.createElement("div");
    div.className = `msg ${roleClass}${isUser ? '' : ' md-body'}`;

    if (isUser) {
      div.textContent = content;
    } else {
      div.innerHTML = renderMarkdown(content);
    }

    row.appendChild(div);

    // Message actions toolbar
    if (!isUser) {
      const actions = document.createElement("div");
      actions.className = "msg-actions";
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

    const stepsEl = document.createElement("agent-steps");

    const contentEl = document.createElement("div");
    contentEl.className = "stream-content md-body";

    const typingEl = document.createElement("div");
    typingEl.className = "typing-indicator";
    typingEl.setAttribute("aria-label", "Agent is responding");
    typingEl.innerHTML = "<span></span><span></span><span></span>";

    streamArea.appendChild(stepsEl);
    streamArea.appendChild(typingEl);
    streamArea.appendChild(contentEl);
    streamRow.appendChild(streamArea);
    messagesEl.appendChild(streamRow);
    messagesEl.scrollTop = messagesEl.scrollHeight;

    // Follow the stream only while the user is at the bottom.
    const follow = () => {
      if (nearBottom(messagesEl)) messagesEl.scrollTop = messagesEl.scrollHeight;
    };

    const showContent = (html, { progress = false } = {}) => {
      typingEl.remove();
      contentEl.classList.add("is-visible");
      contentEl.classList.toggle("is-progress", progress);
      contentEl.innerHTML = html;
      follow();
    };

    const renderReply = frameRenderer((text) => {
      stepsEl.finish();
      showContent(renderMarkdown(text));
    });
    const out = await readA2aStream(res, {
      onReply: renderReply,
      // Working prose goes to the activity timeline, not into the message
      // body: it is the agent's tool activity, and rendering it there as a
      // growing blob only to overwrite it with the reply lost the sequence
      // and read as a flicker. `out.progressText` still backs the
      // no-reply fallback below.
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

    // Finalize
    stepsEl.finish();
    typingEl.remove();
    let fullText = out.text;
    if (out.failed && !fullText) {
      fullText = out.errorMessage;
      showContent(`<span style="color:var(--color-error)">${this.#esc(fullText)}</span>`);
    } else if (!fullText) {
      showContent(renderMarkdown("No response"));
      fullText = "No response";
    } else {
      // The frame renderer may still have a queued paint; settle on the
      // final text synchronously so actions append below rendered content.
      showContent(renderMarkdown(fullText));
    }

    // Add actions to stream row
    const actions = document.createElement("div");
    actions.className = "msg-actions";
    actions.innerHTML = `
      <button type="button" class="msg-action-copy" aria-label="Copy message" title="Copy">${icons.copy('', 14)}</button>
      ${usageChipsHtml(out.usage)}
      ${this.#traceLinkHtml(out.traceId)}
    `;
    streamArea.appendChild(actions);

    return { text: fullText, traceId: out.traceId, usage: out.usage };
  }

  // Opens the full Observability session view with this turn's trace
  // preselected — the same page the sessions table links to. It used to point
  // at session-trace.html, a flat span list with no span detail, no
  // attributes and no transcript; that page is now a redirect stub.
  #traceLinkHtml(traceId) {
    if (!traceId) return '';
    const q = new URLSearchParams({ trace_id: traceId });
    if (this.#sessionId) q.set('session_id', this.#sessionId);
    return `<a class="msg-action-trace" href="/observability-session.html?${q}"
      aria-label="View trace" title="View trace">${icons.trace('', 14)}<span>Detailed trace</span></a>`;
  }

  // Assistant rows carry their usage_meta + trace id so chips and the
  // "Detailed trace" link survive a history reload.
  #persistMessage(sessionId, role, content, { traceId = null, usage = null } = {}) {
    const body = { role, content };
    if (traceId || usage) {
      body.usage = {
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
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }).catch(() => {});
  }

  #esc(s) {
    const d = document.createElement("span");
    d.textContent = s || "";
    return d.innerHTML;
  }
}

customElements.define("chat-page", ChatPage);
