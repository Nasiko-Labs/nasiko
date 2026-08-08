import { icons } from '/common/utils/icons.js';

import styles from './agent-steps.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const MAX_TEXT = 2000;

function truncate(s, max = MAX_TEXT) {
  s = s || '';
  return s.length > max ? `${s.slice(0, max)}…` : s;
}

/**
 * <agent-steps> — live tool-call activity for a streamed A2A response.
 *
 * Renders the orchestrator's intermediate events (thinking, tool_call,
 * tool_result, sub_status, sub_content, agent_invoke, agent_result,
 * policy_rejected) as collapsible inline steps, similar to the CLI's
 * per-tool-call output.
 *
 * Usage (see chat-page.js / orchestrator-page.js):
 *   const steps = document.createElement('agent-steps');
 *   container.appendChild(steps);
 *   steps.onEvent(dataPart);   // for each `data` part in the SSE stream
 *   steps.finish();            // when the final answer starts / stream ends
 */
class AgentSteps extends HTMLElement {
  #steps = new Map();
  #count = 0;
  #finished = false;
  #startedAt = null;

  connectedCallback() {
    if (this.querySelector('.steps-header')) return;
    this.#startedAt = Date.now();
    this.innerHTML = `
      <button type="button" class="steps-header" aria-expanded="true">
        ${icons.chevronRight('steps-chevron', 14)}
        <span class="steps-pulse"></span>
        <span class="steps-label">Working…</span>
        <span class="steps-viewall">View all steps</span>
      </button>
      <div class="steps-list"></div>
    `;
    this.querySelector('.steps-header').addEventListener('click', () => {
      const expanded = this.classList.toggle('is-expanded');
      this.querySelector('.steps-header').setAttribute('aria-expanded', String(expanded));
      const viewall = this.querySelector('.steps-viewall');
      if (viewall) viewall.textContent = expanded ? 'Hide steps' : 'View all steps';
    });
    this.classList.add('is-expanded');
  }

  get hasSteps() {
    return this.#count > 0;
  }

  #setLabel(text) {
    const label = this.querySelector('.steps-label');
    if (label) label.textContent = text;
  }

  #addStep(key, { title, sub, request }) {
    this.#count += 1;
    const step = document.createElement('div');
    step.className = 'step is-running';
    step.innerHTML = `
      <button type="button" class="step-summary" aria-expanded="false">
        ${icons.chevronRight('step-chevron', 12)}
        <span class="step-state" aria-hidden="true"></span>
        <span class="step-title"></span>
        <span class="step-sub"></span>
      </button>
      <div class="step-body" hidden></div>
    `;
    step.querySelector('.step-title').textContent = title;
    step.querySelector('.step-sub').textContent = truncate(sub || '', 120);
    step.querySelector('.step-summary').addEventListener('click', () => {
      const body = step.querySelector('.step-body');
      const open = step.classList.toggle('is-open');
      body.hidden = !open;
      step.querySelector('.step-summary').setAttribute('aria-expanded', String(open));
    });
    if (request) this.#addSection(step, 'Request', request);
    this.querySelector('.steps-list').appendChild(step);
    this.#steps.set(key, step);
    return step;
  }

  #addSection(step, name, text) {
    const section = document.createElement('div');
    section.className = 'step-section';
    section.innerHTML = `<div class="step-k"></div><pre class="step-v"></pre>`;
    section.querySelector('.step-k').textContent = name;
    section.querySelector('.step-v').textContent = truncate(text);
    step.querySelector('.step-body').appendChild(section);
    return section;
  }

  #openStepFor(agent) {
    // Latest still-running step for this agent (sub_* events carry no turn).
    let found = null;
    for (const step of this.#steps.values()) {
      if (step.dataset.agent === agent && step.classList.contains('is-running')) found = step;
    }
    return found;
  }

  #completeStep(step, { success, result }) {
    if (!step) return;
    step.classList.remove('is-running');
    step.classList.add(success ? 'is-ok' : 'is-fail');
    if (!result) return;
    // Skip the Response section when it duplicates the streamed Output
    // (agents often send the same text via sub_content and tool_result).
    const streamed = step.querySelector('.step-stream');
    const norm = (s) => s.replace(/\s+/g, ' ').trim();
    if (streamed && norm(streamed.textContent).includes(norm(truncate(result)))) return;
    if (streamed && norm(truncate(result)).startsWith(norm(streamed.textContent).slice(0, 200))) {
      // Response supersedes a truncated/partial stream: replace instead of duplicating.
      streamed.textContent = truncate(result);
      const label = streamed.closest('.step-section')?.querySelector('.step-k');
      if (label) label.textContent = 'Response';
      return;
    }
    this.#addSection(step, 'Response', result);
  }

  /** Handle one `data` part from the A2A status stream. */
  onEvent(d) {
    if (!d || !d.type) return;
    switch (d.type) {
      case 'thinking':
        this.#setLabel(d.content ? truncate(d.content, 120) : 'Thinking…');
        break;
      case 'tool_call': {
        const step = this.#addStep(`turn:${d.turn}`, {
          title: d.agent,
          sub: d.message,
          request: d.message,
        });
        step.dataset.agent = d.agent;
        this.#setLabel(`Calling ${d.agent}…`);
        break;
      }
      case 'tool_result': {
        const step = this.#steps.get(`turn:${d.turn}`) || this.#openStepFor(d.agent);
        this.#completeStep(step, { success: d.success !== false, result: d.result });
        this.#setLabel(`${d.agent} responded`);
        break;
      }
      case 'sub_status': {
        const step = this.#openStepFor(d.agent);
        if (step && d.message) {
          // One live line per step, updated in place — appending a new node
          // per SSE event stacked every chunk on its own line.
          let live = step.querySelector('.step-live-line');
          if (!live) {
            live = document.createElement('div');
            live.className = 'step-live-line';
            step.querySelector('.step-body').appendChild(live);
          }
          live.textContent = truncate(d.message, 200);
        }
        if (d.message) this.#setLabel(`${d.agent}: ${truncate(d.message, 100)}`);
        break;
      }
      case 'sub_content': {
        const step = this.#openStepFor(d.agent);
        if (step && d.content) {
          let pre = step.querySelector('.step-stream');
          if (!pre) {
            pre = this.#addSection(step, 'Output', '').querySelector('.step-v');
            pre.classList.add('step-stream');
            pre.textContent = '';
          }
          if (pre.textContent.length < MAX_TEXT) pre.textContent += d.content;
        }
        break;
      }
      case 'agent_invoke': {
        const step = this.#addStep(`invoke:${d.target_agent}:${this.#count}`, {
          title: d.target_agent,
          sub: `called by ${d.caller_agent}`,
        });
        step.dataset.agent = d.target_agent;
        this.#setLabel(`${d.caller_agent} calling ${d.target_agent}…`);
        break;
      }
      case 'agent_result': {
        const step = this.#openStepFor(d.target_agent);
        this.#completeStep(step, { success: true });
        this.#setLabel(`${d.target_agent} responded`);
        break;
      }
      case 'policy_rejected': {
        const step = this.#addStep(`policy:${this.#count}`, {
          title: d.agent,
          sub: 'blocked by policy',
          request: d.reason,
        });
        this.#completeStep(step, { success: false });
        this.#setLabel(`${d.agent} blocked: ${truncate(d.reason || '', 80)}`);
        break;
      }
    }
  }

  /**
   * Settle the component: stop the pulse, mark stragglers done, collapse
   * the list, and hide entirely when no steps were recorded. Idempotent.
   */
  finish() {
    if (this.#finished) return;
    this.#finished = true;
    if (!this.#count) {
      this.style.display = 'none';
      return;
    }
    for (const step of this.#steps.values()) {
      if (step.classList.contains('is-running')) {
        step.classList.remove('is-running');
        step.classList.add('is-ok');
      }
    }
    this.classList.remove('is-expanded');
    this.classList.add('is-done');
    const header = this.querySelector('.steps-header');
    if (header) header.setAttribute('aria-expanded', 'false');
    const pulse = this.querySelector('.steps-pulse');
    if (pulse) pulse.outerHTML = icons.checkCircle('steps-check', 15);
    const secs = this.#startedAt ? (Date.now() - this.#startedAt) / 1000 : null;
    const routedTo = [...this.#steps.values()].find((s) => s.dataset.agent)?.dataset.agent;
    const label = this.querySelector('.steps-label');
    if (label) {
      const strong = document.createElement('span');
      strong.className = 'steps-strong';
      strong.textContent = secs != null ? `Reasoned for ${secs.toFixed(1)}s` : 'Done';
      const meta = document.createElement('span');
      meta.className = 'steps-meta';
      meta.textContent = ` ${this.#count} step${this.#count === 1 ? '' : 's'}${routedTo ? ` · routed to ${routedTo}` : ''}`;
      label.replaceChildren(strong, meta);
    }
  }
}

customElements.define('agent-steps', AgentSteps);
