import { Marked } from '/common/vendor/marked.esm.js';
import DOMPurify from '/common/vendor/dompurify.esm.js';
import hljs from '/common/vendor/highlight.esm.js';
import { icons } from '/common/utils/icons.js';

/**
 * Markdown renderer for LLM output, shared by chat + orchestrator pages.
 *
 * Pipeline: marked (GFM: tables, nested lists, strikethrough, ...) →
 * highlight.js for fenced code blocks → DOMPurify sanitize.
 *
 * Usage:
 *   container.classList.add('md-body');       // styles: /common/styles/markdown.css
 *   container.innerHTML = renderMarkdown(text);
 *
 * Code blocks emit a header with a copy button; bind a delegated click
 * handler for `.md-code-copy` (see chat-page.js / orchestrator-page.js).
 */

function escapeHtml(text) {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/**
 * Syntax-highlight code, falling back to escaped plain text for unknown
 * languages or highlighter errors. Returns HTML-safe markup either way.
 */
function highlightCode(code, language) {
  if (language && hljs.getLanguage(language)) {
    try {
      return hljs.highlight(code, { language, ignoreIllegals: true }).value;
    } catch {
      // fall through to plain rendering
    }
  }
  return escapeHtml(code);
}

// Dedicated instance so we don't mutate the shared `marked` singleton.
const marked = new Marked({
  gfm: true,
  breaks: true, // single newlines become <br>, matches LLM chat conventions
  renderer: {
    code({ text, lang }) {
      const language = (lang || '').split(/\s+/)[0].toLowerCase();
      return (
        `<div class="md-code-block">` +
        `<div class="md-code-header">` +
        `<span class="md-code-lang">${escapeHtml(language) || 'code'}</span>` +
        `<button type="button" class="md-code-copy" aria-label="Copy code">${icons.copy('', 14)}</button>` +
        `</div>` +
        `<pre><code>${highlightCode(text, language)}</code></pre>` +
        `</div>`
      );
    },
    codespan({ text }) {
      return `<code class="md-inline-code">${escapeHtml(text)}</code>`;
    },
  },
});

// LLM output is untrusted: force links to open in a new tab without a
// window.opener reference. DOMPurify strips target/rel otherwise.
DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName === 'A' && node.hasAttribute('href')) {
    node.setAttribute('target', '_blank');
    node.setAttribute('rel', 'noopener noreferrer');
  }
});

/**
 * Render untrusted markdown to sanitized HTML.
 * @param {string} text
 * @returns {string} HTML safe to assign to innerHTML
 */
export function renderMarkdown(text) {
  if (!text) return '';
  let html;
  try {
    html = marked.parse(text);
  } catch {
    // Never let a parser edge case blank out a chat message.
    html = `<p>${escapeHtml(text)}</p>`;
  }
  return DOMPurify.sanitize(html);
}
