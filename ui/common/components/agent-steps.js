import { icons } from '/common/utils/icons.js';
import { renderMarkdown } from '/common/utils/markdown.js';

import styles from './agent-steps.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

/** Long payloads clamp behind "Show more" rather than being cut with an ellipsis. */
const CLAMP_CHARS = 4000;

/**
 * <agent-steps> — live activity timeline for a streamed A2A response.
 *
 * **Capability-adaptive by design.** The A2A stream carries whatever the agent
 * chose to send, and that differs per agent:
 *
 *  - Agents that emit structured data parts (`tool_name` + `arguments` +
 *    `result`, in any of the `tool*`/`function*` spellings below) get real tool
 *    rows: pretty-printed JSON input, rendered output, per-call duration.
 *  - Agents that only put prose in their WORKING status (every seed agent
 *    today) get that prose as an ordered activity log under the agent row —
 *    styled the same, just without the input/output detail nobody sent.
 *
 * So richer agents render richer, and nothing is invented for the rest.
 *
 * Event vocabulary (oss/react-agent/src/events.rs → data parts in
 * a2a_dispatch.rs): thinking · tool_call · tool_result · sub_status ·
 * sub_content · agent_invoke · agent_result · policy_rejected. In the
 * orchestrator every "tool" IS an agent (A2aTool), so `tool_call` renders as
 * an agent row; genuine tool rows come from the structured parts above.
 *
 * Usage (chat-page.js / orchestrator-page.js):
 *   const steps = document.createElement('agent-steps');
 *   steps.onEvent(dataPart);   // per `data` part in the SSE stream
 *   steps.finish();            // when the final answer starts / stream ends
 */
class AgentSteps extends HTMLElement {
  #rows = new Map();      // key -> row element
  #agents = 0;
  #tools = 0;
  #finished = false;
  #startedAt = null;
  #ticker = null;

  connectedCallback() {
    if (this.querySelector('.steps-header')) return;
    this.#startedAt = Date.now();
    this.innerHTML = `
      <button type="button" class="steps-header" aria-expanded="true">
        ${icons.chevronRight('steps-chevron', 14)}
        <span class="steps-pulse" aria-hidden="true"></span>
        <span class="steps-label">Working…</span>
        <span class="steps-elapsed" aria-hidden="true"></span>
        <span class="steps-viewall">Hide steps</span>
      </button>
      <div class="steps-list" role="list"></div>
    `;
    this.querySelector('.steps-header').addEventListener('click', () => {
      const expanded = this.classList.toggle('is-expanded');
      this.querySelector('.steps-header').setAttribute('aria-expanded', String(expanded));
      this.querySelector('.steps-viewall').textContent = expanded ? 'Hide steps' : 'View all steps';
    });
    this.classList.add('is-expanded');
    // One shared ticker drives every running row's elapsed time, so a long
    // run doesn't accumulate a timer per row.
    this.#ticker = setInterval(() => this.#tick(), 200);
  }

  disconnectedCallback() {
    clearInterval(this.#ticker);
  }

  get hasSteps() {
    return this.#rows.size > 0;
  }

  // ── Row construction ──────────────────────────────────────────────────────

  /**
   * @param kind 'agent' | 'tool' | 'thinking' | 'policy'
   * @param parent optional row to nest under (tools nest inside their agent)
   */
  #addRow(key, { kind, title, subtitle, parent }) {
    const row = document.createElement('div');
    row.className = `step step--${kind} is-running`;
    row.dataset.startedAt = String(Date.now());
    row.setAttribute('role', 'listitem');
    row.innerHTML = `
      <button type="button" class="step-summary" aria-expanded="false">
        ${icons.chevronRight('step-chevron', 12)}
        <span class="step-icon" aria-hidden="true">${this.#kindIcon(kind)}</span>
        <span class="step-title"></span>
        <span class="step-sub"></span>
        <span class="step-time" aria-hidden="true"></span>
        <span class="step-state" aria-hidden="true"></span>
      </button>
      <div class="step-body" hidden></div>
    `;
    row.querySelector('.step-title').textContent = title;
    row.querySelector('.step-sub').textContent = subtitle || '';
    row.querySelector('.step-summary').addEventListener('click', () => this.#toggle(row));

    const host = parent ? this.#childrenOf(parent) : this.querySelector('.steps-list');
    host.appendChild(row);
    this.#rows.set(key, row);
    if (kind === 'agent') this.#agents += 1;
    if (kind === 'tool') this.#tools += 1;
    return row;
  }

  #toggle(row) {
    const body = row.querySelector('.step-body');
    const open = row.classList.toggle('is-open');
    body.hidden = !open;
    row.querySelector('.step-summary').setAttribute('aria-expanded', String(open));
  }

  /** Nested rows live in the parent's body so they collapse with it. */
  #childrenOf(parent) {
    let nest = parent.querySelector(':scope > .step-body > .step-children');
    if (!nest) {
      nest = document.createElement('div');
      nest.className = 'step-children';
      parent.querySelector('.step-body').appendChild(nest);
      // A parent with children is worth opening by default while running.
      if (!parent.classList.contains('is-open')) this.#toggle(parent);
    }
    return nest;
  }

  #kindIcon(kind) {
    if (kind === 'tool') return icons.terminal('', 13);
    if (kind === 'thinking') return icons.brain('', 13);
    if (kind === 'policy') return icons.shield('', 13);
    return icons.cube('', 13);
  }

  // ── Body sections ─────────────────────────────────────────────────────────

  /**
   * Render a payload the best way it can be read: JSON gets pretty-printed in a
   * code well, prose gets markdown, and anything long clamps behind a toggle.
   */
  #addSection(row, name, value, { markdown = false } = {}) {
    const section = document.createElement('div');
    section.className = 'step-section';
    const label = document.createElement('div');
    label.className = 'step-k';
    label.textContent = name;
    section.appendChild(label);

    const json = this.#asJson(value);
    let content;
    if (json !== null) {
      content = document.createElement('pre');
      content.className = 'step-v step-v--code';
      content.textContent = JSON.stringify(json, null, 2);
    } else if (markdown) {
      content = document.createElement('div');
      content.className = 'step-v step-v--md md-body';
      content.innerHTML = renderMarkdown(String(value ?? ''));
    } else {
      content = document.createElement('pre');
      content.className = 'step-v';
      content.textContent = String(value ?? '');
    }

    const text = String(value ?? '');
    if (text.length > CLAMP_CHARS) content.classList.add('is-clamped');
    section.appendChild(content);
    if (text.length > CLAMP_CHARS) section.appendChild(this.#moreToggle(content));

    row.querySelector('.step-body').appendChild(section);
    return content;
  }

  #moreToggle(target) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'step-more';
    btn.textContent = 'Show more';
    btn.addEventListener('click', () => {
      const open = target.classList.toggle('is-expanded');
      btn.textContent = open ? 'Show less' : 'Show more';
    });
    return btn;
  }

  /** Parsed JSON when the value really is JSON (object/array), else null. */
  #asJson(value) {
    if (value && typeof value === 'object') return value;
    if (typeof value !== 'string') return null;
    const t = value.trim();
    if (!t.startsWith('{') && !t.startsWith('[')) return null;
    try {
      const parsed = JSON.parse(t);
      return parsed && typeof parsed === 'object' ? parsed : null;
    } catch {
      return null;
    }
  }

  // ── State transitions ─────────────────────────────────────────────────────

  #settle(row, { success = true, blocked = false } = {}) {
    if (!row || !row.classList.contains('is-running')) return;
    row.classList.remove('is-running');
    row.classList.add(blocked ? 'is-blocked' : success ? 'is-ok' : 'is-fail');
    const started = Number(row.dataset.startedAt);
    if (!row.dataset.durationMs && started) {
      row.dataset.durationMs = String(Date.now() - started);
    }
    this.#paintTime(row);
  }

  #paintTime(row) {
    const el = row.querySelector('.step-time');
    if (!el) return;
    const ms = row.dataset.durationMs
      ? Number(row.dataset.durationMs)
      : Date.now() - Number(row.dataset.startedAt);
    el.textContent = ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`;
  }

  #tick() {
    for (const row of this.#rows.values()) {
      if (row.classList.contains('is-running')) this.#paintTime(row);
    }
    if (!this.#finished) {
      const el = this.querySelector('.steps-elapsed');
      if (el) el.textContent = `${((Date.now() - this.#startedAt) / 1000).toFixed(1)}s`;
    }
  }

  #setLabel(text) {
    const label = this.querySelector('.steps-label');
    if (label) label.textContent = text;
  }

  /**
   * Newest still-running agent row (sub_* events carry no turn). With no
   * `agent` argument, any running agent row — used by `onActivity`, whose
   * lines carry no attribution.
   */
  #openAgentRow(agent) {
    let found = null;
    for (const row of this.#rows.values()) {
      if (!row.classList.contains('is-running')) continue;
      if (!row.classList.contains('step--agent')) continue;
      if (agent === undefined || row.dataset.agent === agent) found = row;
    }
    return found;
  }

  /**
   * Structured tool payload, accepting the spellings agents actually use.
   * Returns null when the part carries no tool identity — those fall through
   * to the prose path instead of being rendered as an empty tool row.
   */
  #toolShape(d) {
    const name = d.tool_name || d.tool || d.name || d.function_name;
    if (!name) return null;
    return {
      name,
      args: d.arguments ?? d.args ?? d.input ?? d.parameters ?? null,
      result: d.result ?? d.output ?? d.response ?? null,
      success: d.success !== false && !d.error,
      durationMs: d.duration_ms ?? null,
      agent: d.agent || d.agent_name || null,
      id: d.id || d.tool_call_id || null,
    };
  }

  // ── Event entry point ─────────────────────────────────────────────────────

  /**
   * One line of working prose from the agent (A2A WORKING status text).
   *
   * This is the *only* activity channel most agents use — they relay tool
   * work as text ("web_search: <query>"), not as structured data parts. It
   * lands under the agent currently running when there is one (orchestrator
   * routing), otherwise in a single top-level Activity row (a direct agent
   * chat, where there is no agent hop to nest under).
   */
  onActivity(line) {
    if (!line) return;
    const parent = this.#openAgentRow();

    // The agent SDK reports each tool as `tool_name: <input>` (a real
    // infra-agent stream sends `dns_lookup: example.com`), so promote that
    // shape to a proper tool row with its input. Anything else stays a log
    // line — no guessing.
    const tool = /^([a-z][a-z0-9_.-]{1,48}):[ \t]+(\S.*)$/i.exec(line);
    if (tool) {
      const [, name, input] = tool;
      // A repeat of the same tool is a new call, not an update: key by
      // occurrence so `ip_info` twice renders as two rows.
      const key = `act:${name}:${input}`;
      if (!this.#rows.has(key)) {
        // The previous activity tool row is finished once the next one starts;
        // agents report starts only, never completions, on this channel.
        this.#settleActivityRows();
        const row = this.#addRow(key, { kind: 'tool', title: name, subtitle: input, parent });
        row.dataset.activity = 'true';
        this.#addSection(row, 'Input', input);
        this.#setLabel(`${name}…`);
      }
      return;
    }

    const row = parent
      || this.#rows.get('activity')
      || this.#addRow('activity', { kind: 'tool', title: 'Activity' });
    let log = row.querySelector(':scope > .step-body > .step-log');
    if (!log) {
      log = document.createElement('div');
      log.className = 'step-log';
      row.querySelector('.step-body').appendChild(log);
      if (!row.classList.contains('is-open')) this.#toggle(row);
    }
    const el = document.createElement('div');
    el.className = 'step-log-line';
    el.textContent = line;
    log.appendChild(el);
    this.#setLabel(line);
  }

  /** Activity-derived tool rows have no completion event; the next one ends them. */
  #settleActivityRows() {
    for (const row of this.#rows.values()) {
      if (row.dataset.activity === 'true') this.#settle(row);
    }
  }

  /** Handle one `data` part from the A2A status stream. */
  onEvent(d) {
    if (!d || !d.type) return;
    const type = String(d.type);

    // Structured tool activity, whatever the wrapper type is called.
    if (/tool|function/.test(type) && this.#toolShape(d)) {
      this.#onToolEvent(type, this.#toolShape(d));
      return;
    }

    switch (type) {
      case 'thinking': {
        const row = this.#rows.get('thinking')
          || this.#addRow('thinking', { kind: 'thinking', title: 'Thinking' });
        const body = row.querySelector('.step-body');
        if (!body.querySelector('.step-section')) this.#addSection(row, 'Reasoning', d.content || '', { markdown: true });
        else body.querySelector('.step-v').innerHTML = renderMarkdown(d.content || '');
        this.#settle(row);
        this.#setLabel('Thinking…');
        break;
      }
      case 'tool_call': {
        // Orchestrator "tools" are agents.
        const row = this.#addRow(`turn:${d.turn}`, {
          kind: 'agent',
          title: d.agent,
          subtitle: d.message,
        });
        row.dataset.agent = d.agent;
        if (d.message) this.#addSection(row, 'Request', d.message, { markdown: true });
        this.#setLabel(`Calling ${d.agent}…`);
        break;
      }
      case 'tool_result': {
        const row = this.#rows.get(`turn:${d.turn}`) || this.#openAgentRow(d.agent);
        if (row && d.duration_ms != null) row.dataset.durationMs = String(d.duration_ms);
        this.#completeAgent(row, d);
        this.#setLabel(`${d.agent} responded`);
        break;
      }
      case 'sub_status': {
        // Prose activity from an agent that sends no structured tool parts.
        // Appended as a log, not overwritten — the sequence is the story.
        const row = this.#openAgentRow(d.agent);
        if (row && d.message) {
          let log = row.querySelector('.step-log');
          if (!log) {
            log = document.createElement('div');
            log.className = 'step-log';
            row.querySelector('.step-body').appendChild(log);
          }
          const line = document.createElement('div');
          line.className = 'step-log-line';
          line.textContent = d.message;
          log.appendChild(line);
        }
        if (d.message) this.#setLabel(`${d.agent}: ${d.message}`);
        break;
      }
      case 'sub_content': {
        const row = this.#openAgentRow(d.agent);
        if (row && d.content) {
          let stream = row.querySelector('.step-stream');
          if (!stream) {
            stream = this.#addSection(row, 'Output', '', { markdown: true });
            stream.classList.add('step-stream');
            stream.dataset.raw = '';
          }
          stream.dataset.raw += d.content;
          stream.innerHTML = renderMarkdown(stream.dataset.raw);
        }
        break;
      }
      case 'agent_invoke': {
        const row = this.#addRow(`invoke:${d.target_agent}:${this.#rows.size}`, {
          kind: 'agent',
          title: d.target_agent,
          subtitle: `called by ${d.caller_agent}`,
        });
        row.dataset.agent = d.target_agent;
        this.#setLabel(`${d.caller_agent} → ${d.target_agent}…`);
        break;
      }
      case 'agent_result': {
        this.#settle(this.#openAgentRow(d.target_agent));
        this.#setLabel(`${d.target_agent} responded`);
        break;
      }
      case 'policy_rejected': {
        const row = this.#addRow(`policy:${this.#rows.size}`, {
          kind: 'policy',
          title: d.agent,
          subtitle: 'blocked by flow policy',
        });
        if (d.reason) this.#addSection(row, 'Reason', d.reason);
        this.#settle(row, { blocked: true });
        this.#setLabel(`${d.agent} blocked`);
        break;
      }
    }
  }

  /** A structured tool call/result, nested under its agent when known. */
  #onToolEvent(type, t) {
    const key = `tool:${t.id || `${t.agent || ''}:${t.name}`}`;
    const finishing = /result|response|end|finish|output/.test(type) || t.result != null;

    let row = this.#rows.get(key);
    if (!row) {
      const parent = t.agent ? this.#openAgentRow(t.agent) : null;
      row = this.#addRow(key, {
        kind: 'tool',
        title: t.name,
        subtitle: this.#argPreview(t.args),
        parent,
      });
      if (t.agent) row.dataset.agent = t.agent;
      if (t.args != null) this.#addSection(row, 'Input', t.args);
      this.#setLabel(`${t.name}…`);
    }
    if (!finishing) return;

    if (t.result != null) this.#addSection(row, 'Output', t.result, { markdown: true });
    if (t.durationMs != null) row.dataset.durationMs = String(t.durationMs);
    this.#settle(row, { success: t.success });
    this.#setLabel(`${t.name} done`);
  }

  /** One-line argument summary for the collapsed row. */
  #argPreview(args) {
    if (args == null) return '';
    const obj = this.#asJson(args);
    if (!obj) return String(args);
    if (Array.isArray(obj)) return JSON.stringify(obj);
    return Object.entries(obj)
      .map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`)
      .join(' · ');
  }

  #completeAgent(row, d) {
    if (!row) return;
    const result = d.result;
    if (result) {
      // Don't repeat the streamed Output verbatim as a Response section.
      const streamed = row.querySelector('.step-stream');
      const norm = (s) => (s || '').replace(/\s+/g, ' ').trim();
      const already = streamed && norm(streamed.dataset.raw).includes(norm(result).slice(0, 200));
      if (!already) this.#addSection(row, 'Response', result, { markdown: true });
    }
    this.#settle(row, { success: d.success !== false });
  }

  /**
   * Settle the component: stop timers, mark stragglers done, collapse, and
   * hide entirely when nothing was recorded. Idempotent.
   */
  finish() {
    if (this.#finished) return;
    this.#finished = true;
    clearInterval(this.#ticker);
    if (!this.#rows.size) {
      this.style.display = 'none';
      return;
    }
    for (const row of this.#rows.values()) this.#settle(row);

    this.classList.remove('is-expanded');
    this.classList.add('is-done');
    this.querySelector('.steps-header')?.setAttribute('aria-expanded', 'false');
    this.querySelector('.steps-viewall').textContent = 'View all steps';
    const pulse = this.querySelector('.steps-pulse');
    if (pulse) pulse.outerHTML = icons.checkCircle('steps-check', 15);
    const elapsed = this.querySelector('.steps-elapsed');
    if (elapsed) elapsed.remove();

    const secs = (Date.now() - this.#startedAt) / 1000;
    const parts = [];
    if (this.#agents) parts.push(`${this.#agents} agent${this.#agents === 1 ? '' : 's'}`);
    if (this.#tools) parts.push(`${this.#tools} tool${this.#tools === 1 ? '' : 's'}`);
    const label = this.querySelector('.steps-label');
    if (label) {
      const strong = document.createElement('span');
      strong.className = 'steps-strong';
      strong.textContent = `Reasoned for ${secs.toFixed(1)}s`;
      const meta = document.createElement('span');
      meta.className = 'steps-meta';
      meta.textContent = parts.length ? ` ${parts.join(' · ')}` : '';
      label.replaceChildren(strong, meta);
    }
  }
}

customElements.define('agent-steps', AgentSteps);
