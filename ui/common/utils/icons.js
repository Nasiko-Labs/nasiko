/**
 * Central SVG icon library.
 *
 * Each icon is a function: (cls = '', size = N, weight = 1.5) => SVG string
 *
 *   import { icons } from '../utils/icons.js';
 *   // in a template:
 *   `<button>${icons.search('my-class')}</button>`
 *   `<span>${icons.x('', 16)}</span>`
 *   `<span>${icons.search('', 16, 1)}</span>`  // chrome weight (topbar/rail)
 *
 * NightOwl weight rule: topbar chrome renders at 1px stroke, white-plane icons
 * at the default 1.5px. The module rail is the exception at 1.75 — at 18px a
 * 1.25 stroke resolved to under a physical pixel and the rail read as washed
 * out beside the page content it labels.
 *
 * `icons.google` is a static string (multicolour brand mark, fixed 18×18).
 */

const s = (body, defaultSize = 24) =>
  (cls = '', size = defaultSize, weight = 1.5) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" ` +
    `stroke-width="${weight}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" ` +
    `style="width:${size}px;height:${size}px;flex-shrink:0"` +
    `${cls ? ` class="${cls}"` : ''}>${body}</svg>`;

// Bold-stroke variant (stroke-width 2.5) — used for small control icons
const sb = (body, defaultSize = 24) =>
  (cls = '', size = defaultSize) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" ` +
    `stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" ` +
    `style="width:${size}px;height:${size}px;flex-shrink:0"` +
    `${cls ? ` class="${cls}"` : ''}>${body}</svg>`;

const f = (body, viewBox, defaultSize = 24) =>
  (cls = '', size = defaultSize) =>
    `<svg width="${size}" height="${size}" viewBox="${viewBox}" fill="currentColor" aria-hidden="true" ` +
    `style="width:${size}px;height:${size}px;flex-shrink:0"` +
    `${cls ? ` class="${cls}"` : ''}>${body}</svg>`;

export const icons = {
  // ── Navigation ───────────────────────────────────────────────────────────
  search: s(`<path d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"/>`),
  menu:   s(`<path d="M4 6h16M4 12h16M4 18h16"/>`),

  chevronDown:  s(`<path d="M19 9l-7 7-7-7"/>`),
  history: s(`<path d="M3 12a9 9 0 1 0 3.5-7.1L3 8"/><path d="M3 3v5h5"/><polyline points="12 7 12 12 15 14"/>`),
  chevronUp:    s(`<path d="M5 15l7-7 7 7"/>`),
  chevronRight: s(`<path d="M9 5l7 7-7 7"/>`),
  chevronLeft:  s(`<path d="M15 18l-6-6 6-6"/>`),

  // GitHub Primer-style filled small chevron (16×16)
  chevronDownSmall: f(
    `<path d="M4.427 7.427l3.396 3.396a.25.25 0 00.354 0l3.396-3.396A.25.25 0 0011.396 7H4.604a.25.25 0 00-.177.427z"/>`,
    '0 0 16 16',
    16
  ),

  // Sidebar toggle (panel-left icon from Lucide)
  panelLeft: s(`<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/>`),

  // Wide arc arrows used in calendar navigation
  arrowLeft:  s(`<path d="M15.5 3.5l-7 8.5 7 8.5"/>`),
  arrowRight: s(`<path d="M8.5 3.5l7 8.5-7 8.5"/>`),

  // ── Actions ──────────────────────────────────────────────────────────────
  x:    s(`<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>`),
  check: s(`<polyline points="20 6 9 17 4 12"/>`),
  send:  s(`<line x1="22" y1="2" x2="11" y2="13"/><polygon points="22 2 15 22 11 13 2 9 22 2"/>`),
  arrowUp: s(`<path d="M12 19V5"/><path d="M5 12l7-7 7 7"/>`),
  arrowDown: s(`<path d="M12 5v14"/><path d="M19 12l-7 7-7-7"/>`),
  arrowUpRight: s(`<line x1="7" y1="17" x2="17" y2="7"/><polyline points="7 7 17 7 17 17"/>`),
  refresh:       s(`<path d="M23 4v6h-6"/><path d="M1 20v-6h6"/><path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>`),
  cloudDownload: s(`<polyline points="8 17 12 21 16 17"/><line x1="12" y1="12" x2="12" y2="21"/><path d="M20.88 18.09A5 5 0 0018 9h-1.26A8 8 0 103 16.29"/>`),
  square:  s(`<rect x="3" y="3" width="18" height="18" rx="2" ry="2" fill="currentColor" stroke="none"/>`),
  mic:     f(`<path d="M12 14c1.66 0 2.99-1.34 2.99-3L15 5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3zm5.3-3c0 3-2.54 5.1-5.3 5.1S6.7 14 6.7 11H5c0 3.41 2.72 6.23 6 6.72V21h2v-3.28c3.28-.48 6-3.3 6-6.72h-1.7z"/>`, '0 0 24 24'),
  micStop: f(`<rect x="6" y="6" width="12" height="12" rx="2"/>`, '0 0 24 24'),
  paperclip: s(`<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"/>`),
  copy:  s(`<rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>`),

  // Pencil-only edit icon
  edit: s(`<path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z"/>`),

  // Thin-stroke pencil (for inline edit affordances) — Tabler style
  editThin: (cls = '', size = 24) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" ` +
    `stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" ` +
    `style="width:${size}px;height:${size}px;flex-shrink:0"${cls ? ` class="${cls}"` : ''}>` +
    `<path d="M4 20h4l10.5-10.5a2.828 2.828 0 1 0-4-4L4 16v4z"/>` +
    `<path d="M13.5 6.5l4 4"/></svg>`,

  // Pencil + checkbox outline (custom action / "write to doc" concept)
  editInBox: s(`<polyline points="9 11 12 14 22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/>`),

  // ── Status ───────────────────────────────────────────────────────────────
  checkCircle: s(`<path d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/>`),
  xCircle:     s(`<circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/>`),
  info:        s(`<circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/>`),
  faceFrown:   s(`<path d="M9.172 16.172a4 4 0 015.656 0M9 10h.01M15 10h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/>`),

  // ── Files & content ──────────────────────────────────────────────────────
  document: s(`<path d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"/>`),
  folder:   s(`<path d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"/>`),
  briefcase: s(`<rect x="2" y="7" width="20" height="14" rx="2" ry="2"/><path d="M16 21V5a2 2 0 0 0-2-2h-4a2 2 0 0 0-2 2v16"/>`),
  eye:      s(`<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>`),
  calendar: s(`<rect x="3" y="4" width="18" height="18" rx="2" ry="2"/><line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/>`),
  clock:    s(`<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>`),
  // NightOwl "access" key — round bow with a dot, straight shaft, two teeth
  key:      s(`<circle cx="7.8" cy="12" r="4.6"/><circle cx="7.8" cy="12" r=".4" fill="currentColor" stroke="none"/><path d="M12.4 12h8.35"/><path d="M17.4 12v3.1"/><path d="M20.75 12v3.1"/>`),
  lock:     s(`<rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>`),
  shield:   s(`<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>`),

  // File icons for folder-browser (paths only; caller wraps in <svg>)
  filePaths: {
    default: 'M7 21h10a2 2 0 002-2V9.414a1 1 0 00-.293-.707l-5.414-5.414A1 1 0 0012.586 3H7a2 2 0 00-2 2v14a2 2 0 002 2z',
    code:    'M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8l-6-6zM6 20V4h7v5h5v11H6z',
    image:   'M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z',
  },

  trash:   s(`<polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/>`),
  sparkles: s(`<path d="M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z"/><path d="M20 3v4"/><path d="M22 5h-4"/>`),
  circle:  s(`<circle cx="12" cy="12" r="9"/>`),
  loader:  s(`<path d="M21 12a9 9 0 1 1-6.219-8.56"/>`),
  plus:    s(`<line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>`),
  play:    s(`<polygon points="5 3 19 12 5 21 5 3" fill="currentColor" stroke="none"/>`),
  runBacktest: s(`<circle cx="12" cy="12" r="10"/><polygon points="10 8 16 12 10 16 10 8" fill="currentColor" stroke="currentColor" stroke-linejoin="round" stroke-width="1.5"/>`),
  logOut:  s(`<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/>`),
  moreVertical: s(`<circle cx="12" cy="12" r="1"/><circle cx="12" cy="5" r="1"/><circle cx="12" cy="19" r="1"/>`),
  sun:     s(`<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/>`),
  moon:    s(`<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>`),
  monitor: s(`<rect x="2" y="3" width="20" height="14" rx="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/>`),
  // NightOwl "vision" eye — browed eye with pupil (mockup Observability glyph)
  // An eye inside four corner brackets — the viewfinder/focus motif, i.e.
  // "something is under observation". The brackets are the four rounded corners
  // of a square, drawn as separate strokes so the frame reads as deliberate
  // framing rather than a box.
  //
  // Two earlier attempts were worse and are worth not repeating: loose arcs
  // floating above an off-centre eye read as unrelated strokes, and a dashed
  // ring read as a *broken* circle at 18px, because the dash gaps land near a
  // single physical pixel at rail size.
  activity: s(`<path d="M3 8.5V6a3 3 0 0 1 3-3h2.5"/><path d="M15.5 3H18a3 3 0 0 1 3 3v2.5"/><path d="M21 15.5V18a3 3 0 0 1-3 3h-2.5"/><path d="M8.5 21H6a3 3 0 0 1-3-3v-2.5"/><path d="M6.6 12s2.2-3.05 5.4-3.05S17.4 12 17.4 12s-2.2 3.05-5.4 3.05S6.6 12 6.6 12z"/><circle cx="12" cy="12" r="1.5"/>`),
  user:    s(`<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>`),
  users:   s(`<path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>`),
  externalLink: s(`<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/>`),
  cornerUpRight: s(`<polyline points="15 14 20 9 15 4"/><path d="M4 20v-7a4 4 0 0 1 4-4h12"/>`),

  // ── Table / data controls (bold-stroke for small sizes) ──────────────────
  sortBoth: sb(`<path d="M8 6l4-4 4 4"/><path d="M8 18l4 4 4-4"/>`, 14),
  sortAsc:  sb(`<path d="M8 6l4-4 4 4"/>`, 14),
  sortDesc: sb(`<path d="M8 18l4 4 4-4"/>`, 14),
  pageNext: sb(`<path d="M9 18l6-6-6-6"/>`, 16),
  pagePrev: sb(`<path d="M15 18l-6-6 6-6"/>`, 16),
  // ── Checkbox (task-list) ──────────────────────────────────────────
  checkboxChecked: (cls = '', size = 16) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"` +
    ` class="task-checkbox task-checkbox--checked${cls ? ' ' + cls : ''}"` +
    ` aria-label="checked">` +
    `<rect x="1" y="1" width="14" height="14" rx="2"/><polyline points="3.5,8 6.5,11 12.5,5"/></svg>`,

  checkboxUnchecked: (cls = '', size = 16) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"` +
    ` class="task-checkbox task-checkbox--unchecked${cls ? ' ' + cls : ''}"` +
    ` aria-label="unchecked">` +
    `<rect x="1" y="1" width="14" height="14" rx="2"/></svg>`,

  // ── Dev ──────────────────────────────────────────────────────────────────
  trace: s(`<circle cx="6" cy="19" r="3"/><path d="M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15"/><circle cx="18" cy="5" r="3"/>`),
  code: s(`<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>`),
  terminal: s(`<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>`),

  // ── Add Agent methods ─────────────────────────────────────────────────────
  github:  f(`<path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/>`, '0 0 24 24'),
  upload:  s(`<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/>`),
  layers:  s(`<polygon points="12 2 2 7 12 12 22 7 12 2"/><polyline points="2 17 12 22 22 17"/><polyline points="2 12 12 17 22 12"/>`),
  // Three linked nodes. The previous shape was an org chart of squares, which
  // read as "hierarchy" rather than "connected things".
  network: s(`<circle cx="17.8" cy="5.8" r="2.7"/><circle cx="6.2" cy="12" r="2.7"/><circle cx="17.8" cy="18.2" r="2.7"/><path d="M8.6 10.7 15.4 7.1"/><path d="M8.6 13.3 15.4 16.9"/>`),
  // NightOwl module glyphs (drawn from the mockup's Hugeicons-style set:
  // Home06 / AiBrain01 / Vision / Puzzle / Money03 / Setting07)
  // A robot with an antenna and a face. The previous shape was a featureless
  // rounded shell that read as a house or a shield at rail size — nothing in it
  // said "agent".
  bot: s(`<circle cx="12" cy="2.6" r="1.15"/><path d="M12 3.75v1.9"/><rect x="3.6" y="5.65" width="16.8" height="12.6" rx="4.2"/><circle cx="8.9" cy="11.1" r="1.05" fill="currentColor" stroke="none"/><circle cx="15.1" cy="11.1" r="1.05" fill="currentColor" stroke="none"/><path d="M9.3 14.35a3.5 3.5 0 0 0 5.4 0"/><path d="M1.9 10.6v2.7"/><path d="M22.1 10.6v2.7"/>`),
  // Two stacked racks with status lights. The previous shape was a puzzle-piece
  // blob — legible as "plugin", not as "server".
  server: s(`<rect x="3.2" y="4.5" width="17.6" height="6.2" rx="2"/><rect x="3.2" y="13.3" width="17.6" height="6.2" rx="2"/><circle cx="7" cy="7.6" r=".95" fill="currentColor" stroke="none"/><circle cx="7" cy="16.4" r=".95" fill="currentColor" stroke="none"/><path d="M10.6 7.6h6.4"/><path d="M10.6 16.4h6.4"/>`),
  route: s(`<circle cx="6" cy="19" r="3"/><path d="M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15"/><circle cx="18" cy="5" r="3"/>`),
  coins: s(`<circle cx="8" cy="8" r="6"/><path d="M18.09 10.37A6 6 0 1 1 10.34 18"/><path d="M7 6h1v4"/><path d="m16.71 13.88.7.71-2.82 2.82"/>`),
  banknote: s(`<path d="M2.75 8.1c1.9-1.5 4-1.5 6-.55 2.05.98 4.45.98 6.5 0 2-.95 4.1-.95 6 .55v7.8c-1.9-1.5-4-1.5-6-.55-2.05.98-4.45.98-6.5 0-2-.95-4.1-.95-6 .55V8.1z"/><circle cx="12" cy="11.95" r="2.15"/><path d="M6.4 12.85h.01"/><path d="M17.6 11.05h.01"/>`),
  workflow: s(`<rect width="8" height="8" x="3" y="3" rx="2"/><path d="M7 11v4a2 2 0 0 0 2 2h4"/><rect width="8" height="8" x="13" y="13" rx="2"/>`),
  settings: s(`<path d="M9.36 5.63 9.95 2.83A1.1 1.1 0 0 1 14.05 2.83L14.64 5.63 17.04 4.06A1.1 1.1 0 0 1 19.94 6.96L18.37 9.36 21.17 9.95A1.1 1.1 0 0 1 21.17 14.05L18.37 14.64 19.94 17.04A1.1 1.1 0 0 1 17.04 19.94L14.64 18.37 14.05 21.17A1.1 1.1 0 0 1 9.95 21.17L9.36 18.37 6.96 19.94A1.1 1.1 0 0 1 4.06 17.04L5.63 14.64 2.83 14.05A1.1 1.1 0 0 1 2.83 9.95L5.63 9.36 4.06 6.96A1.1 1.1 0 0 1 6.96 4.06Z"/><circle cx="12" cy="12" r="2.6"/>`),
  brain: s(`<path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96.44 2.5 2.5 0 0 1-2.96-3.08 3 3 0 0 1-.34-5.58 2.5 2.5 0 0 1 1.32-4.24 2.5 2.5 0 0 1 1.98-3A2.5 2.5 0 0 1 9.5 2Z"/><path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96.44 2.5 2.5 0 0 0 2.96-3.08 3 3 0 0 0 .34-5.58 2.5 2.5 0 0 0-1.32-4.24 2.5 2.5 0 0 0-1.98-3A2.5 2.5 0 0 0 14.5 2Z"/>`),
  bell: s(`<path d="M18.4 9.7a6.4 6.4 0 1 0-12.8 0c0 3.3-.9 5-1.7 5.9-.5.55-.75.83-.73 1.13.02.34.09.47.36.67.24.2.66.2 1.5.2h14.94c.84 0 1.26 0 1.5-.2.27-.2.34-.33.36-.67.02-.3-.23-.58-.73-1.13-.8-.9-1.7-2.6-1.7-5.9z"/><path d="M9.6 20.6a2.7 2.7 0 0 0 4.8 0"/>`),
  sourceCode: s(`<path d="M7.2 8.2 3.4 12l3.8 3.8"/><path d="M16.8 8.2 20.6 12l-3.8 3.8"/><path d="M13.7 5.6l-3.4 12.8"/>`),
  cube:    s(`<path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"/><polyline points="3.27 6.96 12 12.01 20.73 6.96"/><line x1="12" y1="22.08" x2="12" y2="12"/>`),

  // ── Brand ────────────────────────────────────────────────────────────────
  // Static multicolour mark — not a function (size/colour are fixed by brand guidelines)
  google: `<svg width="18" height="18" viewBox="0 0 18 18" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">` +
    `<path d="M17.64 9.2c0-.637-.057-1.251-.164-1.84H9v3.481h4.844c-.209 1.125-.843 2.078-1.796 2.717v2.258h2.908c1.702-1.567 2.684-3.874 2.684-6.615z" fill="#4285F4"/>` +
    `<path d="M9 18c2.43 0 4.467-.806 5.956-2.18l-2.908-2.259c-.806.54-1.837.86-3.048.86-2.344 0-4.328-1.584-5.036-3.711H.957v2.332A8.997 8.997 0 0 0 9 18z" fill="#34A853"/>` +
    `<path d="M3.964 10.71A5.41 5.41 0 0 1 3.682 9c0-.593.102-1.17.282-1.71V4.958H.957A8.996 8.996 0 0 0 0 9c0 1.452.348 2.827.957 4.042l3.007-2.332z" fill="#FBBC05"/>` +
    `<path d="M9 3.58c1.321 0 2.508.454 3.44 1.345l2.582-2.58C13.463.891 11.426 0 9 0A8.997 8.997 0 0 0 .957 4.958L3.964 6.29C4.672 4.163 6.656 3.58 9 3.58z" fill="#EA4335"/>` +
    `</svg>`,

  // Official four-square Microsoft mark (brand guideline colours, fixed 18×18)
  microsoft: `<svg width="18" height="18" viewBox="0 0 21 21" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">` +
    `<rect x="1" y="1" width="9" height="9" fill="#F25022"/>` +
    `<rect x="11" y="1" width="9" height="9" fill="#7FBA00"/>` +
    `<rect x="1" y="11" width="9" height="9" fill="#00A4EF"/>` +
    `<rect x="11" y="11" width="9" height="9" fill="#FFB900"/>` +
    `</svg>`,
};

// Canonical NightOwl names for the module glyphs above — the nav data still
// references the legacy names (bot/activity/server/banknote), so both resolve
// to the same drawing until navigation.js is renamed.
icons.home = icons.bot;
icons.vision = icons.activity;
icons.puzzle = icons.server;
icons.money = icons.banknote;