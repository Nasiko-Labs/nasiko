/**
 * Minimal ANSI SGR → HTML converter for container/build log lines.
 *
 * Container logs come straight from agent stdout, where tracing frameworks
 * emit color escapes (e.g. `\x1b[2m` dim, `\x1b[32m` green). Instead of
 * showing those as `[2m…[0m` noise, render them as spans; unknown escape
 * sequences are stripped.
 *
 * Input is HTML-escaped first, so the result is safe for innerHTML.
 */

const FG = {
  30: 'var(--color-text-main)',
  31: 'light-dark(#d73a49, #ff7b72)', // red
  32: 'light-dark(#22863a, #7ee787)', // green
  33: 'light-dark(#b08800, #e3b341)', // yellow
  34: 'light-dark(#005cc5, #79c0ff)', // blue
  35: 'light-dark(#6f42c1, #d2a8ff)', // magenta
  36: 'light-dark(#0598a8, #56d4dd)', // cyan
  37: 'var(--color-text-muted)',
};
// Bright variants reuse the same palette.
for (let i = 90; i <= 97; i++) FG[i] = FG[i - 60];

function escapeHtml(text) {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

// ESC[ params letter — SGR uses the letter `m`.
const ANSI_RE = /\x1b\[([0-9;]*)([A-Za-z])/g;

/**
 * @param {string} text raw log text, possibly containing ANSI escapes
 * @returns {string} HTML-safe markup with styled spans
 */
export function ansiToHtml(text) {
  if (!text) return '';
  let html = '';
  let open = 0;
  const state = { color: null, dim: false, bold: false, italic: false, underline: false };

  const closeAll = () => {
    html += '</span>'.repeat(open);
    open = 0;
  };
  const applyState = () => {
    closeAll();
    const styles = [];
    if (state.color) styles.push(`color:${state.color}`);
    if (state.dim) styles.push('opacity:0.6');
    if (state.bold) styles.push('font-weight:600');
    if (state.italic) styles.push('font-style:italic');
    if (state.underline) styles.push('text-decoration:underline');
    if (styles.length) {
      html += `<span style="${styles.join(';')}">`;
      open = 1;
    }
  };

  let last = 0;
  for (const m of text.matchAll(ANSI_RE)) {
    html += escapeHtml(text.slice(last, m.index));
    last = m.index + m[0].length;
    if (m[2] !== 'm') continue; // strip non-SGR sequences (cursor moves, etc.)
    const codes = (m[1] || '0').split(';').map(Number);
    for (const c of codes) {
      if (c === 0) {
        state.color = null;
        state.dim = state.bold = state.italic = state.underline = false;
      } else if (c === 1) state.bold = true;
      else if (c === 2) state.dim = true;
      else if (c === 3) state.italic = true;
      else if (c === 4) state.underline = true;
      else if (c === 22) { state.bold = false; state.dim = false; }
      else if (c === 23) state.italic = false;
      else if (c === 24) state.underline = false;
      else if (c === 39) state.color = null;
      else if (FG[c]) state.color = FG[c];
      // 38;5;n / 38;2;r;g;b extended colors are ignored (next codes harmless)
    }
    applyState();
  }
  html += escapeHtml(text.slice(last));
  closeAll();
  return html;
}
