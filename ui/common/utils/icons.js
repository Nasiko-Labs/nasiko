/**
 * Central SVG icon library.
 *
 * Each icon is a function: (cls = '', size = N) => SVG string
 *
 *   import { icons } from '../utils/icons.js';
 *   // in a template:
 *   `<button>${icons.search('my-class')}</button>`
 *   `<span>${icons.x('', 16)}</span>`
 *
 * `icons.google` is a static string (multicolour brand mark, fixed 18×18).
 */

const s = (body, defaultSize = 24) =>
  (cls = '', size = defaultSize) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" ` +
    `stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" ` +
    `style="width:${size}px;height:${size}px;flex-shrink:0"` +
    `${cls ? ` class="${cls}"` : ''}>${body}</svg>`;

// Bold-stroke variant (stroke-width 2.5) — used for small control icons
const sb = (body, defaultSize = 24) =>
  (cls = '', size = defaultSize) =>
    `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" ` +
    `stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" ` +
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
  eye:      s(`<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>`),
  calendar: s(`<rect x="3" y="4" width="18" height="18" rx="2" ry="2"/><line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/>`),
  clock:    s(`<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>`),
  key:      s(`<path d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 0-7.778 7.778 5.5 5.5 0 0 0 7.777 0L15.5 15.5m0 0l3 3L21 16l-3-3"/>`),
  lock:     s(`<rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>`),

  // File icons for folder-browser (paths only; caller wraps in <svg>)
  filePaths: {
    default: 'M7 21h10a2 2 0 002-2V9.414a1 1 0 00-.293-.707l-5.414-5.414A1 1 0 0012.586 3H7a2 2 0 00-2 2v14a2 2 0 002 2z',
    code:    'M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8l-6-6zM6 20V4h7v5h5v11H6z',
    image:   'M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z',
  },

  trash:   s(`<polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/><path d="M10 11v6M14 11v6"/><path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/>`),
  plus:    s(`<line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>`),
  play:    s(`<polygon points="5 3 19 12 5 21 5 3" fill="currentColor" stroke="none"/>`),
  runBacktest: s(`<circle cx="12" cy="12" r="10"/><polygon points="10 8 16 12 10 16 10 8" fill="currentColor" stroke="currentColor" stroke-linejoin="round" stroke-width="1.5"/>`),
  logOut:  s(`<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/>`),
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
  trace: s(`<polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/>`),
  code: s(`<polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/>`),

  // ── Add Agent methods ─────────────────────────────────────────────────────
  github:  f(`<path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/>`, '0 0 24 24'),
  upload:  s(`<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/>`),
  layers:  s(`<polygon points="12 2 2 7 12 12 22 7 12 2"/><polyline points="2 17 12 22 22 17"/><polyline points="2 12 12 17 22 12"/>`),
  settings: s(`<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09a1.65 1.65 0 0 0 1.51-1.08 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9c.26.604.852.997 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>`),
  cube:    s(`<path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"/><polyline points="3.27 6.96 12 12.01 20.73 6.96"/><line x1="12" y1="22.08" x2="12" y2="12"/>`),

  // ── Brand ────────────────────────────────────────────────────────────────
  // Static multicolour mark — not a function (size/colour are fixed by brand guidelines)
  google: `<svg width="18" height="18" viewBox="0 0 18 18" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">` +
    `<path d="M17.64 9.2c0-.637-.057-1.251-.164-1.84H9v3.481h4.844c-.209 1.125-.843 2.078-1.796 2.717v2.258h2.908c1.702-1.567 2.684-3.874 2.684-6.615z" fill="#4285F4"/>` +
    `<path d="M9 18c2.43 0 4.467-.806 5.956-2.18l-2.908-2.259c-.806.54-1.837.86-3.048.86-2.344 0-4.328-1.584-5.036-3.711H.957v2.332A8.997 8.997 0 0 0 9 18z" fill="#34A853"/>` +
    `<path d="M3.964 10.71A5.41 5.41 0 0 1 3.682 9c0-.593.102-1.17.282-1.71V4.958H.957A8.996 8.996 0 0 0 0 9c0 1.452.348 2.827.957 4.042l3.007-2.332z" fill="#FBBC05"/>` +
    `<path d="M9 3.58c1.321 0 2.508.454 3.44 1.345l2.582-2.58C13.463.891 11.426 0 9 0A8.997 8.997 0 0 0 .957 4.958L3.964 6.29C4.672 4.163 6.656 3.58 9 3.58z" fill="#EA4335"/>` +
    `</svg>`,
};