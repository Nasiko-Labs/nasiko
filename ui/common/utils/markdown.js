import { marked } from '/common/vendor/marked.esm.js';
import DOMPurify from '/common/vendor/dompurify.esm.js';
import { icons } from '/common/utils/icons.js';

/**
 * Markdown renderer for LLM output, shared by chat + orchestrator pages.
 * marked (GFM: tables, nested lists, strikethrough, ...) → DOMPurify sanitize.
 * Styles live in /common/styles/markdown.css — give the container the
 * `md-body` class. Code blocks emit a header with a copy button; bind a
 * delegated click handler for `.md-code-copy` (see chat-page.js).
 */

function escapeHtml(text) {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

marked.use({
  gfm: true,
  breaks: true, // single newlines become <br>, matches LLM chat conventions
  renderer: {
    code({ text, lang }) {
      const language = (lang || '').split(/\s+/)[0];
      return `<div class="md-code-block"><div class="md-code-header"><span class="md-code-lang">${escapeHtml(language) || 'code'}</span><button type="button" class="md-code-copy" aria-label="Copy code">${icons.copy('', 14)}</button></div><pre><code>${escapeHtml(text)}</code></pre></div>`;
    },
    codespan({ text }) {
      return `<code class="md-inline-code">${escapeHtml(text)}</code>`;
    },
  },
});

// Open links in a new tab; DOMPurify strips target attributes otherwise.
DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (node.tagName === 'A' && node.hasAttribute('href')) {
    node.setAttribute('target', '_blank');
    node.setAttribute('rel', 'noopener noreferrer');
  }
});

export function renderMarkdown(text) {
  if (!text) return '';
  return DOMPurify.sanitize(marked.parse(text));
}
