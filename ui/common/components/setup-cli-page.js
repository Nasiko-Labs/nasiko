/**
 * Set up Nasiko CLI — four-step onboarding guide (install → connect →
 * create → run/publish) built from labeled command snippets.
 *
 * @element setup-cli-page
 * @note Static content, no API calls. Commands mirror `nasiko --help`
 *       (oss/cli/src/main.rs) and the OSS README quick start.
 * @note Deployments may replace the guide wholesale by defining
 *       `window.setupCliSteps` (same shape as STEPS) in navigation.js —
 *       e.g. to install a prebuilt binary instead of building from source.
 */
import { icons } from '../utils/icons.js';
import './app-code-snippet.js';
import styles from './setup-cli-page.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

const DEFAULT_STEPS = [
  {
    key: 'install',
    label: 'Install',
    intro: 'The Nasiko CLI is a single fast binary. Build it from the open-source tree and put it on your PATH.',
    snippets: [
      { label: 'Build the CLI from source', code: 'cargo build --release -p nasiko' },
      { label: 'Install it on your PATH', code: 'sudo cp target/release/nasiko /usr/local/bin/' },
      { label: 'Verify the install', code: 'nasiko --version' },
    ],
    note: 'Requires the Rust toolchain (1.80+) and Docker (or Podman) for building agent images.',
  },
  {
    key: 'cluster',
    label: 'Connect cluster',
    intro: 'Point the CLI at a Nasiko control plane. All remote commands run against the active cluster.',
    snippets: [
      { label: 'No cluster yet? Start a full local stack', code: 'nasiko up' },
      { label: 'Register a control plane by URL', code: 'nasiko connect https://cp.your-company.com --name production' },
      { label: 'Authenticate against the active cluster', code: 'nasiko auth login' },
      { label: 'List clusters and switch the active one', code: 'nasiko clusters\nnasiko use production' },
    ],
    note: 'Cluster registrations live in <code>~/.nasiko/config.json</code>. Check health any time with <code>nasiko status</code>.',
  },
  {
    key: 'agent',
    label: 'Create your first agent',
    intro: 'Scaffold an A2A agent project from a template — it comes with an AgentCard.json, a Dockerfile, and source stubs.',
    snippets: [
      { label: 'Scaffold interactively (pick a template)', code: 'nasiko new' },
      { label: 'Or scaffold straight from a template', code: 'nasiko new openai my-agent' },
      { label: 'Validate the project structure', code: 'cd my-agent\nnasiko validate' },
    ],
    note: 'Add capabilities with <code>nasiko skill add</code> and regenerate the card with <code>nasiko card</code>.',
  },
  {
    key: 'run',
    label: 'Run and publish',
    intro: 'Test the agent locally over the real A2A protocol, then deploy it to the active cluster.',
    snippets: [
      { label: 'Build and run locally', code: 'nasiko run' },
      { label: 'Chat with it over A2A', code: 'nasiko chat http://localhost:8000 "hello"' },
      { label: 'Deploy to the active cluster', code: 'nasiko deploy .' },
      { label: 'Watch it live', code: 'nasiko ps\nnasiko logs my-agent -f' },
    ],
    note: 'No local Docker? <code>nasiko upload</code> ships the source and the server builds it for you.',
  },
];

class SetupCliPage extends HTMLElement {
  #initialized = false;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    const STEPS = Array.isArray(window.setupCliSteps) && window.setupCliSteps.length
      ? window.setupCliSteps
      : DEFAULT_STEPS;

    this.innerHTML = `
      <button class="close-btn" type="button" aria-label="Close">${icons.x('', 16)}</button>

      <div class="page-head">
        <div>
          <h1 class="title-page">Set up Nasiko CLI</h1>
          <p class="page-sub">Build, test, and publish agents directly from your development environment.</p>
        </div>
      </div>

      <div class="step-tabs" role="tablist">
        ${STEPS.map((s, i) => `
          <button class="step-tab${i === 0 ? ' is-active' : ''}" type="button" role="tab"
            aria-selected="${i === 0}" data-step="${s.key}">
            <span class="step-num">${i + 1}</span>${s.label}
          </button>
        `).join('')}
      </div>

      ${STEPS.map((s, i) => `
        <div class="step-panel${i === 0 ? ' is-active' : ''}" data-panel="${s.key}" role="tabpanel">
          <p class="step-intro">${s.intro}</p>
          <div class="snippet-list">
            ${s.snippets.map((sn) => `<app-code-snippet label="${sn.label}">${this.#esc(sn.code)}</app-code-snippet>`).join('')}
          </div>
          <p class="step-note">${s.note}</p>
        </div>
      `).join('')}

      <div class="guide-banner">
        <p class="guide-title">Need the complete guide?</p>
        <p class="guide-sub">Explore advanced workflows including:</p>
        <ul class="guide-list">
          <li>Cluster management</li>
          <li>Multi-registry publishing</li>
          <li>Local chat testing</li>
          <li>Deployment pipelines</li>
          <li>Agent lifecycle management</li>
        </ul>
      </div>
    `;

    this.querySelector('.close-btn').addEventListener('click', () => history.back());

    this.querySelector('.step-tabs').addEventListener('click', (e) => {
      const tab = e.target.closest('.step-tab');
      if (!tab) return;
      this.querySelectorAll('.step-tab').forEach((t) => {
        t.classList.toggle('is-active', t === tab);
        t.setAttribute('aria-selected', String(t === tab));
      });
      this.querySelectorAll('.step-panel').forEach((p) =>
        p.classList.toggle('is-active', p.dataset.panel === tab.dataset.step));
    });
  }

  #esc(str) {
    return String(str).replace(/[&<>"']/g, (m) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;',
    })[m]);
  }
}

customElements.define('setup-cli-page', SetupCliPage);
