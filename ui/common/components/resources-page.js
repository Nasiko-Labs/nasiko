/**
 * Platform resource usage — host headline numbers plus per-container CPU,
 * memory and IO, grouped into control plane / agent runtime / infra.
 *
 * Admin-only page: the endpoint behind it is gated by `require_admin` because it
 * reveals the deployment's shape and the host's size. The per-agent counterpart
 * (owner-visible) lives on the agent card page instead.
 *
 * @element resources-page
 * @note Data source (see /api/docs):
 *       `window.fetchResourceStats()` → GET /api/observability/resources
 */
import styles from './resources-page.css' with { type: 'css' };
import { icons } from '../utils/icons.js';
import '/common/components/app-skeleton.js';
import '/common/components/app-empty-state.js';
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

/** Refresh cadence. The endpoint caches for 5s server-side, so polling faster
 *  would return the same reading and only add load. */
const POLL_INTERVAL_MS = 5000;

const GROUPS = [
  { key: 'control_plane', label: 'Control plane', icon: 'server' },
  { key: 'agent_runtime', label: 'Agent runtime', icon: 'bot' },
  { key: 'infra', label: 'Infrastructure', icon: 'cube' },
];

class ResourcesPage extends HTMLElement {
  #initialized = false;
  #state = 'loading'; // loading | ready | error
  #error = '';
  #data = null;
  #pollTimer = null;

  connectedCallback() {
    if (this.#initialized) return;
    this.#initialized = true;

    this.innerHTML = `
      <app-module-nav module="observability"></app-module-nav>
      <div class="page-head">
        <div>
          <h1 class="title-page">Resources</h1>
          <p class="page-sub">Live CPU, memory and disk for the control plane, the agents and the supporting infrastructure.</p>
        </div>
        <div class="head-meta" id="head-meta"></div>
      </div>
      <div id="banner"></div>
      <div class="kpi-strip" id="kpi-strip" aria-busy="true">
        ${this.#kpiSkeleton()}
      </div>
      <div class="groups" id="groups">
        <section class="pane"><div class="pane-empty" aria-busy="true"><app-skeleton lines="4"></app-skeleton></div></section>
      </div>
    `;

    this.#load();
    this.#pollTimer = setInterval(() => this.#load(), POLL_INTERVAL_MS);
  }

  disconnectedCallback() {
    clearInterval(this.#pollTimer);
    this.#pollTimer = null;
  }

  #kpiSkeleton() {
    return Array.from({ length: 4 })
      .map(
        () => `<div class="kpi">
          <div class="skel-card__line" style="width:80px;"></div>
          <div class="skel-card__line" style="height:20px;width:110px;"></div>
        </div>`,
      )
      .join('');
  }

  async #load() {
    let resp;
    try {
      resp = await window.fetchResourceStats();
    } catch (e) {
      // A 503 here is the normal answer on a Kubernetes or simulated runtime,
      // where usage cannot be read — say so rather than showing zeros.
      this.#state = 'error';
      this.#error = e?.message || 'Resource stats are unavailable.';
      this.#render();
      return;
    }
    this.#data = resp?.data ?? null;
    this.#state = this.#data ? 'ready' : 'error';
    if (!this.#data) this.#error = 'Resource stats are unavailable.';
    this.#render();
  }

  #render() {
    if (this.#state === 'error') {
      // Tear down the success chrome rather than leaving it behind: an emptied
      // KPI strip still draws its two hairlines (reads as a broken render), and a
      // stale "Updated <time>" chip claims a reading we no longer have.
      const strip = this.querySelector('#kpi-strip');
      strip.innerHTML = '';
      strip.hidden = true;
      strip.removeAttribute('aria-busy');
      this.querySelector('#banner').innerHTML = '';
      this.querySelector('#head-meta').innerHTML = '';
      this.querySelector('#groups').innerHTML = `
        <section class="pane">
          <app-empty-state
            title="Resource stats unavailable"
            description="${this.#esc(this.#error)}"
          ></app-empty-state>
        </section>`;
      return;
    }
    // A poll can recover after a failure, so undo the error-state teardown.
    this.querySelector('#kpi-strip').hidden = false;
    this.#renderKpis();
    this.#renderBanner();
    this.#renderGroups();
    this.#renderMeta();
  }

  #renderMeta() {
    const at = this.#data?.collected_at;
    if (!at) return;
    const t = new Date(at);
    const label = Number.isNaN(t.getTime()) ? '' : t.toLocaleTimeString();
    this.querySelector('#head-meta').innerHTML = label
      ? `<span class="chip">${icons.clock('', 12)} Updated ${this.#esc(label)}</span>`
      : '';
  }

  /** The Docker API cannot report host filesystem totals, so say which reading
   *  this is instead of letting a partial disk figure look like the whole truth. */
  #renderBanner() {
    const el = this.querySelector('#banner');
    if (this.#data?.disk_source === 'docker') {
      el.innerHTML = `<div class="banner">${icons.info('', 14)}<span>Disk figures cover the container engine's own images and volumes only. Total host disk needs the host root mounted into the server.</span></div>`;
    } else {
      el.innerHTML = '';
    }
  }

  #renderKpis() {
    const host = this.#data.host || {};
    const containers = this.#allContainers();
    const cpuTotal = containers.reduce((a, c) => a + (c.cpu_percent ?? 0), 0);
    const memUsed = containers.reduce((a, c) => a + (c.mem_used_bytes || 0), 0);

    const cores = host.cpu_count || 0;
    const cpuOfHost = cores > 0 ? cpuTotal / cores : null;
    const memOfHost = host.mem_total_bytes ? (memUsed / host.mem_total_bytes) * 100 : null;

    const diskUsed = host.disk_used_bytes;
    const diskTotal = host.disk_total_bytes;
    const dockerDisk =
      (host.docker_images_bytes || 0) + (host.docker_volumes_bytes || 0);

    const kpis = [
      {
        label: 'CPU in use',
        value: cpuOfHost === null ? '—' : `${cpuOfHost.toFixed(0)}%`,
        sub: cores ? `${this.#fmtCpu(cpuTotal)} of ${cores} core${cores === 1 ? '' : 's'}` : '',
        pct: cpuOfHost,
      },
      {
        label: 'Memory in use',
        value: memOfHost === null ? this.#fmtBytes(memUsed) : `${memOfHost.toFixed(0)}%`,
        sub: host.mem_total_bytes
          ? `${this.#fmtBytes(memUsed)} of ${this.#fmtBytes(host.mem_total_bytes)}`
          : '',
        pct: memOfHost,
      },
      diskTotal
        ? {
            label: 'Disk in use',
            value: `${(((diskUsed || 0) / diskTotal) * 100).toFixed(0)}%`,
            sub: `${this.#fmtBytes(diskUsed || 0)} of ${this.#fmtBytes(diskTotal)}`,
            pct: ((diskUsed || 0) / diskTotal) * 100,
          }
        : {
            label: 'Engine disk',
            value: this.#fmtBytes(dockerDisk),
            sub: `${this.#fmtBytes(host.docker_reclaimable_bytes || 0)} reclaimable`,
            pct: null,
          },
      {
        label: 'Containers',
        value: String(containers.length),
        sub: `${containers.filter((c) => this.#isRunning(c)).length} running`,
        pct: null,
      },
    ];

    const strip = this.querySelector('#kpi-strip');
    strip.removeAttribute('aria-busy');
    strip.innerHTML = kpis
      .map(
        (k) => `
        <div class="kpi">
          <span class="kpi-label">${this.#esc(k.label)}</span>
          <span class="kpi-value">${this.#esc(k.value)}</span>
          ${k.sub ? `<span class="kpi-sub">${this.#esc(k.sub)}</span>` : ''}
          ${k.pct === null ? '' : this.#meterHtml(k.pct)}
        </div>`,
      )
      .join('');
  }

  #renderGroups() {
    const groups = this.#data.groups || {};
    this.querySelector('#groups').innerHTML = GROUPS.map((g) => {
      const list = groups[g.key] || [];
      const icon = icons[g.icon] ? icons[g.icon]('', 14) : '';
      return `
        <section class="pane" aria-label="${this.#esc(g.label)}">
          <h2 class="pane-title">${icon} ${this.#esc(g.label)}
            <span class="pane-count">${list.length} container${list.length === 1 ? '' : 's'}</span>
          </h2>
          ${list.length === 0 ? '<div class="pane-empty">Nothing running in this group.</div>' : this.#rowsHtml(list)}
        </section>`;
    }).join('');
  }

  #rowsHtml(list) {
    const host = this.#data.host || {};
    const cores = host.cpu_count || 0;
    const head = `
      <div class="row row-head" aria-hidden="true">
        <span>Container</span><span>CPU</span><span>Memory</span><span>Network I/O</span>
      </div>`;
    return `<div class="rows">${head}${list.map((c) => this.#rowHtml(c, cores)).join('')}</div>`;
  }

  #rowHtml(c, cores) {
    const running = this.#isRunning(c);
    // Docker reports CPU as percent-of-one-core, so a 2-core box tops out at 200.
    // Both the meter AND the printed value use the host-normalised figure: showing
    // "96%" beside a half-full bar (96% of one core = 48% of two) reads as a bug.
    // The per-core detail moves to the note, where it can be stated in full.
    const known = c.cpu_percent !== null && c.cpu_percent !== undefined;
    const cpuPct = !known ? null : cores > 0 ? c.cpu_percent / cores : c.cpu_percent;
    const coresUsed = known ? c.cpu_percent / 100 : null;
    const memPct = c.mem_limit_bytes
      ? (c.mem_used_bytes / c.mem_limit_bytes) * 100
      : null;

    return `
      <div class="row">
        <div class="cell-name">
          <span class="name-text" title="${this.#esc(c.name)}">${this.#esc(c.display_name || c.name)}</span>
          <span class="chip ${running ? 'is-running' : 'is-stopped'}">${this.#esc(c.state || 'unknown')}</span>
        </div>
        <div class="metric">
          <span class="metric-value">${known ? this.#fmtCpu(cpuPct) : 'not reporting'}</span>
          ${this.#meterHtml(cpuPct)}
          ${known && cores > 0 ? `<span class="metric-note">${coresUsed.toFixed(2)} of ${cores} core${cores === 1 ? '' : 's'}</span>` : ''}
        </div>
        <div class="metric">
          <span class="metric-value">${this.#fmtBytes(c.mem_used_bytes || 0)}</span>
          ${this.#meterHtml(memPct)}
          ${c.mem_limit_bytes ? `<span class="metric-note">of ${this.#fmtBytes(c.mem_limit_bytes)}</span>` : ''}
        </div>
        <div class="metric">
          <span class="metric-value">↓ ${this.#fmtBytes(c.net_rx_bytes || 0)}</span>
          <span class="metric-note">↑ ${this.#fmtBytes(c.net_tx_bytes || 0)}</span>
        </div>
      </div>`;
  }

  /** Meter for a single ratio against a limit. `null` renders an explicitly
   *  unknown track rather than an empty (i.e. "idle") one. */
  #meterHtml(pct) {
    if (pct === null || pct === undefined || Number.isNaN(pct)) {
      return `<div class="meter is-unknown" role="img" aria-label="not reporting"></div>`;
    }
    const clamped = Math.max(0, Math.min(100, pct));
    const sev = clamped >= 90 ? 'is-crit' : clamped >= 70 ? 'is-warn' : 'is-ok';
    const state = clamped >= 90 ? 'critical' : clamped >= 70 ? 'high' : 'normal';
    return `<div class="meter ${sev}" role="img" aria-label="${clamped.toFixed(0)} percent, ${state}">
      <div class="meter-fill" style="width:${clamped.toFixed(1)}%"></div>
    </div>`;
  }

  #allContainers() {
    const g = this.#data?.groups || {};
    return [...(g.control_plane || []), ...(g.agent_runtime || []), ...(g.infra || [])];
  }

  #isRunning(c) {
    return (c.state || '').toLowerCase() === 'running';
  }

  #fmtCpu(pct) {
    if (pct === null || pct === undefined) return 'not reporting';
    return pct >= 10 ? `${pct.toFixed(0)}%` : `${pct.toFixed(1)}%`;
  }

  #fmtBytes(n) {
    if (!n) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let v = n;
    let i = 0;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i += 1;
    }
    return `${v >= 10 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
  }

  #esc(s) {
    return String(s ?? '').replace(
      /[&<>"']/g,
      (m) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[m],
    );
  }
}

customElements.define('resources-page', ResourcesPage);
