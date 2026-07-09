# Vendored libraries

Single-file ESM builds, committed directly because the UI has no build step or
package manager (see "UI is vanilla JS" in CLAUDE.md). Do not edit these files;
replace them wholesale when upgrading.

| File               | Package        | Version | Source                                                            |
| ------------------ | -------------- | ------- | ----------------------------------------------------------------- |
| `marked.esm.js`    | `marked`       | 15.0.12 | `https://cdn.jsdelivr.net/npm/marked@15.0.12/lib/marked.esm.js`   |
| `dompurify.esm.js` | `dompurify`    | 3.2.6   | `https://cdn.jsdelivr.net/npm/dompurify@3.2.6/dist/purify.es.mjs` |
| `highlight.esm.js` | `highlight.js` | 11.11.1 | `https://cdn.jsdelivr.net/npm/highlight.js@11.11.1/es/common/+esm` (common-languages bundle) |

To upgrade: download the new version from the URL above (bump the version in
the path), strip any trailing `//# sourceMappingURL=...` line (the `.map`
files are not vendored), and update this table.

Consumed by `/common/utils/markdown.js`.
