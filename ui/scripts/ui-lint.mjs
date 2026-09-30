#!/usr/bin/env node
/**
 * Architectural lint for the UI layer (`ui/`).
 *
 * There is no bundler, no type checker and no ESLint here, so every rule in
 * AGENTS.md and ARCHITECTURE.md is currently a convention that a person has to
 * remember. This makes the load-bearing ones mechanical.
 *
 * ─── Why it ratchets instead of failing outright ───────────────────────────
 * A lint that reports 1,200 pre-existing violations on day one gets switched off
 * by the end of the week, and then it protects nothing. So each rule declares how
 * it is enforced:
 *
 *   enforce: 'zero'    — must never occur. Any finding fails.
 *   enforce: 'ratchet' — a recorded baseline count that may go DOWN but never up.
 *
 * The baseline is checked in (`ui-lint-baseline.json`) and is a deliberately
 * uncomfortable document: it is the measured debt, per rule, and every number in
 * it is a thing someone chose not to fix yet. Lower one and re-run with
 * `--update-baseline`.
 *
 * Usage:
 *   node ui/scripts/ui-lint.mjs
 *   node ui/scripts/ui-lint.mjs --update-baseline
 *   node ui/scripts/ui-lint.mjs --rule=layer-direction   # one rule, verbose
 */

import { readFileSync, writeFileSync, existsSync, readdirSync } from 'node:fs';
import { resolve, dirname, relative, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { glob } from 'node:fs/promises';

import {
  editionOf,
  editionLayerOf,
  lintGlobs,
  pageGlobs,
  privateElementNames,
  resolveMount,
  serviceBarrels,
} from './editions.mjs';

const SCRIPTS = dirname(fileURLToPath(import.meta.url));
const UI = resolve(SCRIPTS, '..');
const REPO = resolve(UI, '..');
const BASELINE_PATH = resolve(SCRIPTS, 'ui-lint-baseline.json');

// ── Layer model (ARCHITECTURE.md). Lower number = lower layer. ───────────────
const LAYER = { PLATFORM: 0, DESIGN_SYSTEM: 1, COMPONENT: 2, PAGE: 3, DOMAIN: 4, APPLICATION: 5 };

/*
 * Layers are read from the DIRECTORY, never from a list of filenames.
 *
 * This used to be a hand-written `PRIMITIVES` Set: `ui/common/components/` held
 * primitives, product components and whole pages in one flat folder of 101
 * files, and whether a file counted as design-system depended on someone
 * remembering to add its name to that Set. It had already drifted — nine
 * `app-*` files were absent from it, so they silently linted as COMPONENT.
 *
 * Now the path carries the meaning and drift is not expressible:
 *
 *   tokens/         DESIGN_SYSTEM  values only, no selectors that aren't :root
 *   design-system/  DESIGN_SYSTEM  reusable primitives, zero domain knowledge
 *   features/       COMPONENT      product components; may know about our domain
 *   pages/          PAGE           route targets. Nothing may import one.
 *
 * PAGE exists so that "a page is not a reusable component" is a machine check
 * rather than a convention. If a page turns out to be reusable, extract the
 * reusable part down into features/ and leave the page a thin wrapper.
 */

function layerOf(rel) {
  // Vendored third-party ESM sits below everything: it is a dependency, not a
  // layer of ours. Without this, utils/markdown.js importing marked, and
  // core/element.js importing Lit, both read as upward imports.
  if (rel.includes('/vendor/')) return -1;
  if (rel.startsWith('ui/common/core/') || rel.startsWith('ui/common/services/') ||
      rel.startsWith('ui/common/state/') || rel.startsWith('ui/common/utils/')) {
    return LAYER.PLATFORM;
  }
  if (rel.startsWith('ui/common/tokens/') || rel.startsWith('ui/common/design-system/') ||
      rel.startsWith('ui/common/styles/') || rel === 'ui/common/global.css') {
    return LAYER.DESIGN_SYSTEM;
  }
  // The DSL surface runtime: a product component that composes design-system
  // elements from a model-authored spec. Explicit rather than relying on the
  // default-to-COMPONENT fallback, because what it may import is the point.
  if (rel.startsWith('ui/common/surface/')) return LAYER.COMPONENT;
  if (rel.startsWith('ui/common/features/')) return LAYER.COMPONENT;
  if (rel.startsWith('ui/common/pages/')) return LAYER.PAGE;
  // Per-edition layers come from ui/<edition>/edition.json rather than a list of
  // edition names here, because this file is published to the public repo. An
  // edition's page components typically sit at DOMAIN — they import downward into
  // the shared design system and platform, and nothing in ui/common/ may import
  // them — with the rest of the edition at APPLICATION.
  const declared = editionLayerOf(rel);
  if (declared) {
    const layer = LAYER[declared.toUpperCase().replace(/-/g, '_')];
    if (layer === undefined) throw new Error(`${rel}: edition declares unknown layer "${declared}"`);
    return layer;
  }
  return LAYER.COMPONENT;
}

/**
 * A component stylesheet: one that a JS module adopts, as opposed to the shared
 * sheets in styles/ or the token files. These are the sheets that must be
 * @scope-wrapped, because adopting puts them on the document.
 *
 * Deliberately excludes the per-edition component directories — an edition's
 * page-layout sheet is <link>ed, not adopted, so a page element's box exists at
 * first paint. Those are peers of styles/page-layout.css, which this rule has
 * always skipped.
 * @param {string} rel
 */
function isComponentCss(rel) {
  return rel.startsWith('ui/common/design-system/') ||
         rel.startsWith('ui/common/features/') ||
         rel.startsWith('ui/common/pages/');
}

/** Elements that never have a closing tag, so they never open a level. */
const VOID_ELEMENTS = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img',
  'input', 'link', 'meta', 'param', 'source', 'track', 'wbr']);

/** Elements whose content is text, not markup — a `<` inside is not a tag. */
const RAW_TEXT_ELEMENTS = new Set(['script', 'style']);

/**
 * The direct element children of `<body>`, as `{ tag, attrs, index }`.
 *
 * A real parser would be better and this file deliberately has no dependencies,
 * so it counts depth over start and end tags instead. That is enough for what it
 * reads: the page shells are hand-written, well-formed, and never build markup
 * from a string. Comments are skipped and script/style bodies are jumped over,
 * which are the only two places a stray `<` shows up in practice.
 *
 * @param {string} html
 */
function bodyChildren(html) {
  const bodyTag = html.search(/<body\b/i);
  if (bodyTag === -1) return [];
  const start = html.indexOf('>', bodyTag) + 1;
  const close = html.toLowerCase().indexOf('</body', start);
  const body = html.slice(start, close === -1 ? html.length : close);
  const out = [];
  let depth = 0;
  let skipTo = 0;
  const token = /<!--[\s\S]*?-->|<(\/?)([a-zA-Z][\w-]*)([^>]*)>/g;
  for (const m of body.matchAll(token)) {
    if (m.index < skipTo || m[0].startsWith('<!--')) continue;
    const [, closing, name, attrs] = m;
    const tag = name.toLowerCase();
    if (closing) { depth = Math.max(0, depth - 1); continue; }
    if (depth === 0) out.push({ tag, attrs, index: start + m.index });
    if (VOID_ELEMENTS.has(tag) || attrs.trimEnd().endsWith('/')) continue;
    if (RAW_TEXT_ELEMENTS.has(tag)) {
      const end = body.toLowerCase().indexOf(`</${tag}`, m.index);
      if (end !== -1) { skipTo = end; continue; }
    }
    depth += 1;
  }
  return out;
}


/**
 * Raw colour literals in a source file, ignoring comments. A hex quoted in a
 * comment to explain a token ("sand-500, not white@62%") is documentation, not
 * a hardcoded colour, and flagging it teaches people to delete the comment.
 * @param {string} rel
 * @param {string} source
 */
function findRawColours(rel, source) {
  // Blank out comments in place so byte offsets — and therefore line numbers — hold.
  const bare = source.replace(/\/\*[\s\S]*?\*\/|(^|[^:])\/\/[^\n]*/g, (m) => m.replace(/[^\n]/g, ' '));
  const out = [];
  for (const m of bare.matchAll(/#[0-9a-fA-F]{6}\b|rgba?\(\s*\d+\s*,/g)) {
    out.push({ file: rel, line: lineOf(source, m.index), message: `hardcoded colour ${m[0].trim()}` });
  }
  return out;
}

/** Resolve an import specifier to a repo-relative path, or null if not local. */
function resolveSpec(fromRel, spec) {
  if (spec.startsWith('/common/')) return 'ui/common/' + spec.slice('/common/'.length);
  // Server-mounted specifiers, declared per edition (e.g. `/components/` -> that
  // edition's components dir). Mapping them here is what lets the boundary rule
  // below see a public file reaching for a private edition's mount.
  const mounted = resolveMount(spec);
  if (mounted) return mounted;
  if (spec.startsWith('.')) {
    return relative(REPO, resolve(dirname(resolve(REPO, fromRel)), spec)).replace(/\\/g, '/');
  }
  return null;
}

const IMPORT_RES = [
  /(?:^|\s)(?:import|export)[^'"\n]*from\s*['"]([^'"]+)['"]/gm,
  /(?:^|\s)import\s*['"]([^'"]+)['"]/gm,
  /import\(\s*['"]([^'"]+)['"]/gm,
];

function importsOf(source) {
  const out = new Set();
  for (const re of IMPORT_RES) for (const m of source.matchAll(re)) out.add(m[1]);
  return [...out];
}

const lineOf = (source, index) => source.slice(0, index).split('\n').length;

/**
 * The source with every comment blanked out — same length, same offsets, so a
 * match index still maps to the right line via `lineOf`.
 *
 * Needed by any rule that looks for a CALL rather than an import. The three
 * false positives this was written for were all prose: a docblock example in
 * router.js, and two comments in the EE web tree explaining what app.js awaits. A rule
 * that cannot tell a call from a sentence about a call is a rule people learn
 * to ignore.
 *
 * Strings are left alone deliberately. A `//` inside one would be blanked by a
 * naive stripper and could swallow real code after it; leaving them means the
 * worst case is a false positive on a string that contains a comment-looking
 * sequence AND the pattern being searched for, which no rule here does.
 * @param {string} source
 */
function blankComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/(^|[^:])\/\/[^\n]*/g, (m, pre) => pre + ' '.repeat(m.length - pre.length));
}

/**
 * Every path any route table registers, across editions.
 *
 * Route tables are `{ path: '/x', tag, module }` literals living in an `app.js`
 * (the base table) or a `routes-ext*.js` (one layer of the edition chain — the
 * suffix is per-layer, `routes-ext-ee.js` and so on, see
 * common/core/extension-chain.js), so they are found by filename rather than by
 * naming a directory here — ui/scripts/ is published and must not know the
 * private layout. Matching the suffix matters: when the enterprise table was
 * renamed to `routes-ext-ee.js`, an exact-name match would have quietly stopped
 * collecting seven routes, and `nav-url-must-have-route` below would then have
 * reported every enterprise nav entry as pointing nowhere. Read as text: these
 * modules import browser mount paths ('/common/...') that no loader here can
 * resolve.
 */
let ROUTE_PATHS;
function routePaths() {
  if (ROUTE_PATHS) return ROUTE_PATHS;
  ROUTE_PATHS = new Set();
  const walk = (dir) => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      if (e.name === 'node_modules' || e.name === 'vendor' || e.name === 'tests') continue;
      const full = resolve(dir, e.name);
      if (e.isDirectory()) walk(full);
      else if (e.name === 'app.js' || /^routes-ext(-[a-z0-9]+)?\.js$/.test(e.name)) {
        for (const m of readFileSync(full, 'utf8').matchAll(/\bpath:\s*'([^']+)'/g)) ROUTE_PATHS.add(m[1]);
      }
    }
  };
  walk(UI);
  return ROUTE_PATHS;
}

// ─────────────────────────────────────────────────────────────────────────────
//  Rules
// ─────────────────────────────────────────────────────────────────────────────


/**
 * Every deprecated attribute in the design system, read from the components
 * themselves: `@attr {string} title - (deprecated: use heading)`.
 *
 * Generated, not listed. A hand-kept list is what `layerOf` above already
 * describes going wrong once — it drifted, and drift in a list is invisible.
 * Here the component that deprecates an attribute is the same file that
 * declares it, so the two cannot disagree.
 *
 * @returns {Map<string, Map<string, string>>} tag -> (old attribute -> new one)
 */
function deprecatedAttributes() {
  if (deprecatedAttributes.cache) return deprecatedAttributes.cache;
  const out = new Map();
  const DS = resolve(UI, 'common/design-system');
  for (const dir of readdirSync(DS, { withFileTypes: true })) {
    if (!dir.isDirectory()) continue;
    const file = resolve(DS, dir.name, `${dir.name}.js`);
    let src;
    try { src = readFileSync(file, 'utf8'); } catch { continue; }
    const attrs = new Map();
    for (const m of src.matchAll(/@attr\s*\{[^}]*\}\s*([\w-]+)\s*-\s*\(deprecated:\s*use\s+([\w-]+)\)/g)) {
      attrs.set(m[1], m[2]);
    }
    if (attrs.size) out.set(dir.name, attrs);
  }
  deprecatedAttributes.cache = out;
  return out;
}

const rules = [
  {
    id: 'layer-direction',
    enforce: 'zero',
    why: 'Dependencies must point downward: Application -> Domain -> Components -> Design System -> Platform. ' +
         'An upward import is what makes a component library un-reusable.',
    check({ rel, source, isJs }) {
      if (!isJs) return [];
      const from = layerOf(rel);
      const out = [];
      for (const spec of importsOf(source)) {
        const target = resolveSpec(rel, spec);
        if (!target) continue;
        const to = layerOf(target);
        if (to > from) {
          out.push({
            file: rel,
            line: lineOf(source, source.indexOf(spec)),
            message: `imports ${spec} (layer ${to}) from layer ${from} — that is upward`,
          });
        }
      }
      return out;
    },
  },

  {
    id: 'no-deprecated-attribute',
    enforce: 'zero',
    why: 'A component that renames an attribute keeps the old name working, so nothing breaks and nothing tells ' +
         'you either — the shim warns at runtime, which no CI run reads. app-empty-state\'s `title` was cleaned ' +
         'out of three pages and came back in a fourth written months later against an old example, and the only ' +
         'thing that noticed was a person reading the file. There are 16 deprecated attributes across 12 ' +
         'components; every one of them is the same trap. The pairs are read from the components\' own docblocks, ' +
         'so this rule cannot fall behind the design system.',
    check({ rel, source, isJs }) {
      if (!isJs && !rel.endsWith('.html')) return [];
      const out = [];
      for (const [tag, attrs] of deprecatedAttributes()) {
        if (!source.includes(`<${tag}`)) continue;
        for (const open of source.matchAll(new RegExp(`<${tag}\\b[^>]*>`, 'g'))) {
          for (const [old, replacement] of attrs) {
            if (!new RegExp(`\\b${old}=`).test(open[0])) continue;
            out.push({
              file: rel,
              line: lineOf(source, open.index),
              message: `<${tag}> is given the deprecated "${old}" — use "${replacement}"`,
            });
          }
        }
      }
      return out;
    },
  },

  {
    id: 'one-boot-sequence',
    enforce: 'zero',
    why: 'Every SPA entry point boots the same way — error boundary, routes, exclusions, outlet, router.start, ' +
         'route persistence — and there are four of them (one per app shell). They used to ' +
         'hand-roll that sequence separately, which meant a change to it had to be made four times and was made ' +
         'once. core/create-app.js owns it now; an entry point supplies a config literal. ' +
         'Note what this rule can and cannot see. The enterprise edition manifest puts only two of its source ' +
         'trees in `sources.lint`, so three of the four entry points are outside the lint set — and the browser ' +
         'suite does not serve them either. They are guarded by nothing. Adding them costs 5 findings, all ' +
         'no-private-escape-helper in one of those trees, measured; worth its own change. Until then this rule ' +
         'holds the line for the trees that are linted.',
    check({ rel, source, isJs }) {
      if (!isJs || rel === 'ui/common/core/create-app.js') return [];
      // `router.start(` is the one call that only a boot sequence makes. Route
      // tables, exclusions and the error boundary all have legitimate other
      // callers; starting the router does not.
      const out = [];
      for (const m of blankComments(source).matchAll(/\brouter\.start\s*\(/g)) {
        out.push({
          file: rel,
          line: lineOf(source, m.index),
          message: 'starts the router itself — boot through createApp() from core/create-app.js instead',
        });
      }
      return out;
    },
  },

  {
    id: 'public-must-not-import-private',
    enforce: 'zero',
    why: 'ui/common/ and the published editions are synced to the public repo (see the UI allowlist in ' +
         'scripts/sync-oss.sh). A reference into a private edition there breaks the public build and leaks ' +
         'enterprise code. Renamed from the earlier oss/ee-specific rule, which compared paths against "oss/" ' +
         'and "ee/" while every path it is given is prefixed "ui/" — so it matched nothing and its zero meant ' +
         'nothing. Fixed here.',
    check({ rel, source, isJs }) {
      if (!isJs) return [];
      const from = editionOf(rel);
      if (!rel.startsWith('ui/common/') && !from?.isPublic) return [];
      return importsOf(source)
        .filter((spec) => {
          const target = resolveSpec(rel, spec);
          const to = target ? editionOf(target) : null;
          return to !== null && !to.isPublic;
        })
        .map((spec) => ({
          file: rel,
          line: lineOf(source, source.indexOf(spec)),
          message: `imports ${spec}`,
        }));
    },
  },

  {
    id: 'no-private-escape-helper',
    enforce: 'ratchet',
    why: 'There were 38 of these in four mutually incompatible families, two of which did not escape quotes and ' +
         'were used in attribute position. Import escHtml/escAttr from utils/escape.js, or use a Lit template.',
    check({ rel, source, isJs }) {
      if (!isJs || rel.includes('/utils/escape.js')) return [];
      const re = /(?:^|\s)(?:#?esc(?:ape)?(?:Html|Attr)?)\s*(?:\(|=\s*\()/gm;
      const out = [];
      for (const m of source.matchAll(/^\s*(#?esc\w*)\s*\(([^)]*)\)\s*\{/gm)) {
        out.push({ file: rel, line: lineOf(source, m.index), message: `defines ${m[1]}()` });
      }
      return out;
    },
  },

  {
    id: 'no-window-data-function',
    enforce: 'ratchet',
    why: 'Assigning window.fetchX makes the dependency invisible and resolution order-dependent; a missing name ' +
         'used to render an empty view forever. Register with core/data-sources.js instead.',
    check({ rel, source, isJs }) {
      if (!isJs) return [];
      if (rel.endsWith('.preview.js') || rel.includes('/.preview/')) return [];
      const out = [];
      // The shell contract, not a data source: <app-header> and <app-module-nav>
      // read these by name and they are documented as such in AGENTS.md.
      const SHELL_CONTRACT = new Set(['fetchNavigation', 'fetchModuleNav']);
      for (const m of source.matchAll(/^window\.(\w+)\s*=/gm)) {
        if (SHELL_CONTRACT.has(m[1])) continue;
        if (/^(fetch|delete|update|save|create|generate|run|retry|cancel|transcribe)[A-Z]/.test(m[1])) {
          out.push({ file: rel, line: lineOf(source, m.index), message: `assigns window.${m[1]}` });
        }
      }
      return out;
    },
  },

  {
    id: 'teardown-required',
    enforce: 'zero',
    why: 'A component that registers a document/window listener or an interval without a disconnectedCallback ' +
         'keeps itself (and its closures) alive after removal. Three such leaks existed. NasikoElement handles ' +
         'this via listen()/interval(); hand-written components must do it themselves.',
    check({ rel, source, isJs }) {
      if (!isJs || !rel.includes('/components/')) return [];
      if (!/customElements\.define|extends\s+(HTMLElement|NasikoElement|LitElement)/.test(source)) return [];
      // Only count registrations inside a class body — a module-scope listener has
      // no element to leak (login-page.js does this deliberately).
      const classStart = source.search(/class\s+\w+\s+extends/);
      if (classStart < 0) return [];
      const body = source.slice(classStart);
      const registrations = [...body.matchAll(/\b(?:document|window)\.addEventListener\(|\bsetInterval\(/g)];
      if (!registrations.length) return [];
      if (/disconnectedCallback\s*\(/.test(source)) return [];
      if (/extends\s+NasikoElement/.test(source)) return []; // base class tears down for you
      return [{
        file: rel,
        line: lineOf(source, classStart + registrations[0].index),
        message: `${registrations.length} outside registration(s) in a class with no disconnectedCallback`,
      }];
    },
  },

  {
    id: 'scope-required',
    enforce: 'ratchet',
    why: 'Every component sheet must wrap its rules in @scope (element-name) so specificity stays local ' +
         'without !important. An unscoped component sheet leaks document-wide.',
    check({ rel, source, isJs }) {
      if (isJs || !isComponentCss(rel)) return [];
      if (!source.trim()) return [];
      if (/@scope\s*\(/.test(source)) return [];
      return [{ file: rel, line: 1, message: 'component stylesheet has no @scope wrapper' }];
    },
  },

  {
    id: 'not-defined-belongs-in-not-defined-css',
    enforce: 'zero',
    why: 'A :not(:defined) rule in a component sheet does not exist at first paint (the sheet is adopted by its ' +
         'module), so it cannot reserve geometry — and if it contradicts the linked rule it causes the exact ' +
         'layout shift the contract prevents. app-chatbox cost ~82px of CLS this way.',
    check({ rel, source, isJs }) {
      if (isJs || !isComponentCss(rel)) return [];
      const out = [];
      for (const m of source.matchAll(/:not\(:defined\)/g)) {
        out.push({ file: rel, line: lineOf(source, m.index), message: 'declares :not(:defined) — move it to styles/not-defined.css' });
      }
      return out;
    },
  },

  {
    id: 'no-attach-shadow',
    enforce: 'zero',
    why: 'Every component is light-DOM (core/element.js: 69 of 69, zero attachShadow calls). One shadow root ' +
         'breaks the model in three places at once: the adopted stylesheets in document.adoptedStyleSheets stop ' +
         'applying, @scope wrappers no longer bound anything, and querySelector from a parent stops finding the ' +
         'child. Documented in ARCHITECTURE.md since the start; this is the check that makes it true.',
    check({ rel, source, isJs }) {
      if (!isJs || rel.includes('/vendor/')) return [];
      // Blank comments in place, preserving newlines so line numbers hold. The
      // rule is documented in element.js's own JSDoc, and matching that prose
      // would make the rule fail on the file that states it.
      const bare = source.replace(/\/\*[\s\S]*?\*\/|(^|[^:])\/\/[^\n]*/g, (m) => m.replace(/[^\n]/g, ' '));
      return [...bare.matchAll(/\battachShadow\s*\(/g)].map((m) => ({
        file: rel,
        line: lineOf(bare, m.index),
        message: 'calls attachShadow() — components are light-DOM',
      }));
    },
  },

  {
    id: 'domain-must-not-import-domain',
    enforce: 'ratchet',
    why: 'A page component may import downward into the design system and platform, never sideways into another ' +
         'page component. Sideways is how two pages quietly become one unit that cannot be moved, deleted or ' +
         'published separately — and it is invisible to layer-direction, which only compares layer numbers and so ' +
         'reads DOMAIN -> DOMAIN as level. Shared behaviour belongs in features/ or design-system/. A file may ' +
         'still import its ' +
         'own stylesheet, since that is not a peer module. Baseline 2, both the same shape: departments-page and ' +
         'teams-page each import /components/user-picker.js, a shared widget that happens to live in the ' +
         'page-component directory. It is not a page, so it should not be at this layer — moving it down to a ' +
         'features/ directory clears both at once. Ratcheted rather than zero because that move is a structural ' +
         'decision, and a rule that fails on arrival gets switched off.',
    check({ rel, source, isJs }) {
      if (!isJs || layerOf(rel) !== LAYER.DOMAIN) return [];
      // The page components sit as siblings in one flat directory, so "another
      // directory" does not separate them — the discriminator is another JS
      // module at the same layer. A component's own stylesheet is not a peer.
      return importsOf(source)
        .filter((spec) => {
          const target = resolveSpec(rel, spec);
          return target && target !== rel && target.endsWith('.js') && layerOf(target) === LAYER.DOMAIN;
        })
        .map((spec) => ({
          file: rel,
          line: lineOf(source, source.indexOf(spec)),
          message: `imports peer page component ${spec}`,
        }));
    },
  },

  {
    id: 'no-hardcoded-color',
    enforce: 'zero',
    why: 'Colours must come from design tokens or a re-skin cannot be mechanical. Frozen at zero: the ratchet ran ' +
         'from 67 down to 4, and those last four were third-party brand marks in a preview fixture — not a debt to ' +
         'pay but a category that does not belong to the rule. Fixtures and vendored code are exempt; everything ' +
         'that ships must resolve through var().',
    check({ rel, source, isJs }) {
      if (!editionOf(rel)) return [];
      if (rel.includes('/vendor/') || rel === 'ui/common/global.css') return [];
      // Preview fixtures are inputs to the browser suite, not shipped surface.
      // The four that kept this rule off zero were third-party brand marks
      // (#ea4335, #0acf83, #0078d4, #111111) in mcp.preview.js — a re-skin must
      // NOT recolour someone else's logo, so tokenising them would be wrong.
      // Exempting fixtures is what lets the rule be frozen at zero.
      if (rel.endsWith('.preview.js') || rel.includes('/.preview/')) return [];
      return findRawColours(rel, source);
    },
  },

  {
    id: 'design-system-colors-are-tokens',
    enforce: 'zero',
    why: 'The design system is the layer a re-skin has to be able to ignore. A raw colour in design-system/ or ' +
         'tokens/ means one surface will not follow a palette change, and the failure is invisible until ' +
         'someone re-skins. Raw values belong in exactly two files: tokens/palette.css (the ramps) and ' +
         'tokens/elevation.css (shadow tints, which are alpha on black rather than palette colours). ' +
         'Everything else here must resolve through var().',
    check({ rel, source }) {
      if (!rel.startsWith('ui/common/design-system/') && !rel.startsWith('ui/common/tokens/')) return [];
      if (rel === 'ui/common/tokens/palette.css' || rel === 'ui/common/tokens/elevation.css') return [];
      return findRawColours(rel, source);
    },
  },

  {
    id: 'url-policy',
    enforce: 'zero',
    why: 'A URL is copied, logged, screenshotted and sent to third parties in Referer; in-memory state is not. ' +
         'Write view state with setSearchParams() from utils/url-policy.js, which allowlists the keys.',
    check({ rel, source, isJs }) {
      if (!isJs || rel.includes('url-policy') || rel.endsWith('.preview.js')) return [];
      const out = [];
      for (const m of source.matchAll(/\blocation\.search\s*=|\.searchParams\.set\(\s*['"](\w+)['"]/g)) {
        const key = m[1];
        if (key && ALLOWED_URL_KEYS.has(key)) continue;
        out.push({ file: rel, line: lineOf(source, m.index), message: key ? `writes ?${key}` : 'assigns location.search directly' });
      }
      return out;
    },
  },

  {
    id: 'private-element-in-shared-code',
    enforce: 'ratchet',
    why: 'ui/common/ is synced to the public repo. An element name owned by a private edition, appearing in a ' +
         'shared stylesheet, ships selectors for a page the public binary cannot serve, and is a layering ' +
         'violation the compiler cannot catch. Now zero: those components and their host geometry live in the ' +
         'owning edition. Keep them there. Names come from each manifest\'s "privateElements", so the public ' +
         'repo has nothing to check — it cannot leak what it does not ship.',
    check({ rel, source }) {
      // Stylesheets AND scripts: a private page name in a shared comment is
      // still a private page name published to the public repo, and two of
      // those were sitting in ui/common/ when this rule only read CSS.
      if (!rel.startsWith('ui/common/')) return [];
      const out = [];
      for (const el of PRIVATE_ELEMENTS) {
        const i = source.indexOf(el);
        if (i >= 0) {
          out.push({ file: rel, line: lineOf(source, i), message: `references private-edition element ${el}` });
        }
      }
      return out;
    },
  },

  {
    id: 'service-must-be-in-barrel',
    enforce: 'zero',
    why: 'A service registers its data functions as an import side effect, so a service nothing imports is a ' +
         'service the client router never loads — the page mounts, calls into an empty registry and renders its ' +
         'error state. Standalone documents name their own service; the router path only ever loads the barrel. ' +
         'Every *-service.js beside a barrel must therefore be imported by it. Which editions have a barrel comes ' +
         'from their edition.json "serviceBarrel", not a path literal here: ui/scripts/ is published, so naming an ' +
         'enterprise directory would ship the private layout — and readdirSync on a directory the public repo does ' +
         'not have would throw rather than no-op.',
    check({ rel, source, isJs }) {
      if (!isJs || !SERVICE_BARRELS.includes(rel)) return [];
      const imported = new Set(importsOf(source).map((s) => basename(s)));
      return readdirSync(resolve(REPO, dirname(rel)))
        .filter((f) => f.endsWith('-service.js') && !imported.has(f))
        .map((f) => ({ file: rel, line: 1, message: `does not import ${f} — the router path will not register it` }));
    },
  },

  {
    id: 'edition-seam-needs-a-base-stub',
    enforce: 'zero',
    why: 'The edition overlay seam is a chain of per-layer files — /nav-ext.js, /nav-ext-ee.js, /nav-ext-mt.js ' +
         'and the routes-ext equivalents — and every one of those specifiers is imported unconditionally on ' +
         'every surface. The chain only works because ui/oss, the layer every binary embeds, holds a documented ' +
         'no-op for each link; an overlay then replaces just the file bearing its own suffix. Name a layer in ' +
         'the list without leaving that no-op behind and the import 404s on every surface that has no such ' +
         'overlay, which the loader catches and downgrades to a console warning — so the failure is a layer ' +
         'that silently contributes nothing, which is the exact shape of the bug the chain replaced (NAS-637). ' +
         'Nothing else sees it: check-imports.mjs deliberately does not resolve non-/common/ absolute ' +
         'specifiers, because those are resolved per binary by the rust-embed overlay and not on disk.',
    check({ rel, source, isJs }) {
      // The layer lists live beside the code that folds them — app.js owns the
      // route chain, navigation.js the nav chain.
      if (!isJs || !/(^|\/)(app|navigation)\.js$/.test(rel)) return [];
      const out = [];
      for (const m of blankComments(source).matchAll(/'(\/(?:routes|nav)-ext(?:-[a-z0-9]+)?\.js)'/g)) {
        if (existsSync(resolve(UI, 'oss', m[1].slice(1)))) continue;
        out.push({
          file: rel,
          line: lineOf(source, m.index),
          message: `names layer ${m[1]} but ui/oss${m[1]} does not exist — that link 404s wherever no overlay supplies it`,
        });
      }
      return out;
    },
  },

  {
    id: 'nav-url-must-have-route',
    enforce: 'zero',
    why: 'The nav is a list of URLs and the router is a list of paths, and nothing but this connects them. A nav ' +
         'entry pointing at a path no route table registers is not a 404: the server serves the SPA shell for any ' +
         'clean URL, the router finds no match and returns, and the outlet stays empty — a blank page with working ' +
         'chrome. That is how a page goes blank when its route is renamed, or dropped, and the nav is not. Both ' +
         'sides are plain object literals, so they are matched as text; add the route, or drop the nav entry. ' +
         'Exception: a `.html` URL is never SPA-routed at all — the server serves that literal file directly, so ' +
         'the outlet-stays-empty failure mode this rule guards against cannot happen for one.',
    check({ rel, source, isJs }) {
      if (!isJs || !/(^|\/)(navigation|nav-ext(-[a-z0-9]+)?)\.js$/.test(rel)) return [];
      const routes = routePaths();
      return [...source.matchAll(/\burl:\s*'(\/[^']*)'/g)]
        .filter((m) => !m[1].endsWith('.html') && !routes.has(m[1]))
        .map((m) => ({
          file: rel,
          line: lineOf(source, m.index),
          message: `nav links ${m[1]}, which no route table registers — the router leaves the outlet empty`,
        }));
    },
  },

  {
    id: 'no-css-module-import',
    enforce: 'zero',
    why: 'CSS module scripts — `import styles from \'./x.css\' with { type: \'css\' }` — are Chrome/Edge only. ' +
         'Safari and Firefox reject the attribute while *linking* the module, so the failure is not a missing ' +
         'stylesheet: the whole module graph fails and the app renders a blank page. One such import anywhere in ' +
         'the tree is enough. Nothing else catches it — there is no bundler, tsc ignores import attributes, and ' +
         'the browser tests run Chromium, where it works. Use loadCss() from /common/utils/css.js instead.',
    check({ rel, source, isJs }) {
      if (!isJs) return [];
      return [...source.matchAll(/with\s*\{\s*type:\s*['"]css['"]\s*\}/g)].map((m) => ({
        file: rel,
        line: source.slice(0, m.index).split('\n').length,
        message: "CSS module import — Safari and Firefox fail the whole module graph. Use `const styles = await loadCss(new URL('./x.css', import.meta.url))`.",
      }));
    },
  },

  {
    id: 'no-backtick-in-adopted-sheet',
    enforce: 'zero',
    why: 'Twenty-odd components carry their stylesheet as a template literal passed to replaceSync(). A backtick ' +
         'inside that literal — almost always someone quoting a CSS property in a comment — closes the string early ' +
         'and the whole module stops parsing, so the element never upgrades and every page importing it renders ' +
         'nothing. Nothing else catches it: there is no bundler, tsc does not read the literal, and `node --check` ' +
         'parses the truncated result as valid JS. The browser is the first thing to notice, at runtime, as a bare ' +
         '"missing ) after argument list". Quote CSS in those comments with plain text or single quotes.',
    check({ rel, source, isJs }) {
      if (!isJs) return [];
      const out = [];
      // Only the literal handed straight to replaceSync — a sheet assembled some
      // other way is not this pattern and its interpolations are intentional.
      for (const m of source.matchAll(/replaceSync\(`/g)) {
        const open = m.index + m[0].length;
        // First unescaped backtick after the opening one closes the literal.
        let end = open;
        while (end < source.length) {
          const i = source.indexOf('`', end);
          if (i < 0) return out;
          if (source[i - 1] !== '\\') { end = i; break; }
          end = i + 1;
        }
        // `);` right after it is the well-formed case: the literal ended where
        // the call did. Anything else means a stray backtick closed it early.
        if (!/^\s*\)\s*;?/.test(source.slice(end + 1, end + 4))) {
          out.push({
            file: rel,
            line: lineOf(source, end),
            message: 'backtick inside the replaceSync() stylesheet closes the literal early — the module will not parse',
          });
        }
      }
      return out;
    },
  },
  {
    id: 'weave-renderer-is-createelement-only',
    enforce: 'zero',
    why: 'The Weave surface renderer turns a streamed, model-authored spec into DOM, so every value it handles is ' +
         'untrusted input that arrived over the wire. Its whole security posture is one sentence — createElement and ' +
         'setAttribute, nothing else — and that sentence stays true only for as long as nobody reaches for the faster ' +
         'thing under deadline. innerHTML, insertAdjacentHTML, eval, new Function and any on* handler assignment turn ' +
         'a bad spec from a degraded panel into script execution. A code review will eventually miss one of these; ' +
         'this will not. Build the node, set its attributes, append it.',
    check({ rel, source, isJs }) {
      if (!isJs || !(rel.includes('common/surface/') || rel.includes('common/features/weave-surface/'))) return [];
      const out = [];
      const banned = [
        [/\.innerHTML\s*=/g, 'assigns innerHTML'],
        [/\.outerHTML\s*=/g, 'assigns outerHTML'],
        [/insertAdjacentHTML\s*\(/g, 'calls insertAdjacentHTML'],
        [/\beval\s*\(/g, 'calls eval'],
        [/new\s+Function\s*\(/g, 'calls new Function'],
        [/document\.write\s*\(/g, 'calls document.write'],
        [/setAttribute\s*\(\s*['"`]on/gi, 'sets an on* attribute'],
        [/\.on[a-z]+\s*=\s*(?!null)/g, 'assigns an on* handler property'],
      ];
      for (const [re, message] of banned) {
        for (const m of source.matchAll(re)) out.push({ file: rel, line: lineOf(source, m.index), message });
      }
      return out;
    },
  },

  {
    id: 'weave-imports-every-catalog-element',
    enforce: 'zero',
    why: 'dsl-catalog.json is what a model is ALLOWED to name; the import list in weave-surface.js is what will ' +
         'actually be defined when it does. Two lists, one meaning, and nothing kept them equal — fifteen of ' +
         'forty-one had drifted out, app-tabs and app-segmented-control among them. A generated surface naming a ' +
         'missing one gets an inert element: right tag, no upgrade, no styling, attributes that go nowhere, and no ' +
         'diagnostic, because the renderer did exactly what it was asked. It is also order-dependent — the SPA ' +
         'shares one custom-element registry, so the component is defined if the user happened to visit a page ' +
         'that imports it and undefined on a fresh load straight to /view, which makes the same DSL render two ' +
         'different ways. The file already says it should hold every one of them; this makes that true.',
    check({ rel, source, isJs }) {
      if (!isJs || !rel.endsWith('common/features/weave-surface/weave-surface.js')) return [];
      let catalog;
      try {
        catalog = JSON.parse(readFileSync(resolve(UI, 'common/surface/dsl-catalog.json'), 'utf8'));
      } catch {
        return [{ file: rel, line: 1, message: 'dsl-catalog.json is missing or unreadable' }];
      }
      const imported = new Set(
        [...source.matchAll(/design-system\/([a-z0-9-]+)\/\1\.js/g)].map((m) => m[1]));
      return Object.keys(catalog.components ?? {})
        .filter((tag) => !imported.has(tag))
        .map((tag) => ({
          file: rel,
          line: 1,
          message: `${tag} is in dsl-catalog.json but never imported — a generated surface naming it renders an `
            + 'inert element. Add: '
            + `import '/common/design-system/${tag}/${tag}.js';`,
        }));
    },
  },

  {
    id: 'page-document-marks-its-page',
    enforce: 'zero',
    pages: true,
    why: 'global.css gives the page its white card by selecting `body:has(> app-header) > [data-page]`. That used ' +
         'to be a denylist — every body child that was not the header, the loading bar, a div or a dialog — which ' +
         'assumed we know everything that will ever be a child of <body>. We do not: a QuillBot install appends ' +
         '<qb-toolbar> on focusing any text box, body is a 100dvh flex column, and the toolbar took a card and half ' +
         'the height (the /tokenops card went 763px -> 376px). Grammarly, password managers and translation ' +
         'extensions inject the same way. An allowlist fixes that and moves the cost: a page document that forgets ' +
         'the mark loses its card silently, and nothing catches it until someone opens the page. This catches it.',
    check({ rel, source }) {
      const children = bodyChildren(source);
      // No header means no shell: login and the standalone tenant pages paint
      // their own background and were never cards.
      if (!children.some((c) => c.tag === 'app-header')) return [];
      if (children.some((c) => c.tag === 'main' || /\sdata-page(?=[\s=>/]|$)/.test(c.attrs))) return [];
      const candidate = children.find((c) => c.tag.endsWith('-page'));
      return [{
        file: rel,
        line: candidate ? lineOf(source, candidate.index) : 1,
        message: candidate
          ? `<${candidate.tag}> is the page element but has no data-page`
          : 'no <main> and no body child marked data-page — this page gets no card',
      }];
    },
  },
];

// Declared per edition in edition.json. Empty where no edition declares one,
// which is the correct behaviour in the public repo.
const SERVICE_BARRELS = serviceBarrels();

// Keep in sync with utils/url-policy.js ALLOWED_PARAMS.
const ALLOWED_URL_KEYS = new Set([
  'id', 'agent_id', 'agent_name', 'session_id', 'context_id', 'trace_id', 'span_id',
  'exec', 'name', 'owner', 'tab', 'page', 'period', 'return_to', 'token', 'run_error',
  // Pagination/query params sent to the API, not written to the address bar.
  'limit', 'offset', 'cursor', 'q', 'status',
]);

// Declared by each non-public edition in its edition.json. Empty in the public
// repo, which is correct.
const PRIVATE_ELEMENTS = privateElementNames();

// ─────────────────────────────────────────────────────────────────────────────

// The shared tree, plus whatever each edition declares. Hardcoding the
// per-edition globs here would publish the private layout — see editions.mjs.
const SEARCH = ['ui/common/**/*.{js,css}', ...lintGlobs()];
const SKIP = (p) => p.includes('/vendor/') || p.includes('/tests/') || p.includes('/node_modules/');

const findings = new Map(rules.map((r) => [r.id, []]));
let fileCount = 0;

for (const pattern of SEARCH) {
  for await (const entry of glob(pattern, { cwd: REPO })) {
    const rel = entry.replace(/\\/g, '/');
    if (SKIP(rel)) continue;
    fileCount++;
    const source = readFileSync(resolve(REPO, rel), 'utf8');
    const isJs = /\.(js|mjs)$/.test(rel);
    for (const rule of rules) {
      if (rule.pages) continue;
      for (const f of rule.check({ rel, source, isJs })) findings.get(rule.id).push(f);
    }
  }
}

// Page shells get their own pass. `sources.pages` is a wider set than
// `sources.lint` — an edition can hold page HTML in a directory that was never
// in the lint set — and feeding that wider set to every rule would surface
// findings in markup nobody has looked at. So the pass is opt-in per rule.
const pageRules = rules.filter((r) => r.pages);
if (pageRules.length) {
  const seen = new Set();
  for (const pattern of pageGlobs()) {
    for await (const entry of glob(pattern, { cwd: REPO })) {
      const rel = entry.replace(/\\/g, '/');
      if (SKIP(rel) || seen.has(rel)) continue;
      seen.add(rel);
      const source = readFileSync(resolve(REPO, rel), 'utf8');
      for (const rule of pageRules) {
        for (const f of rule.check({ rel, source, isJs: false })) findings.get(rule.id).push(f);
      }
    }
  }
}

const only = process.argv.find((a) => a.startsWith('--rule='))?.split('=')[1];
const updating = process.argv.includes('--update-baseline');
const baseline = existsSync(BASELINE_PATH) ? JSON.parse(readFileSync(BASELINE_PATH, 'utf8')) : { counts: {} };

if (updating) {
  const counts = {};
  for (const r of rules) counts[r.id] = findings.get(r.id).length;
  writeFileSync(
    BASELINE_PATH,
    JSON.stringify(
      {
        _comment:
          'Measured architectural debt, per rule. Every number here is something someone chose not to fix yet. ' +
          "Counts may go down but never up. Regenerate with `node ui/scripts/ui-lint.mjs --update-baseline`.",
        counts,
      },
      null,
      2,
    ) + '\n',
  );
  console.log('baseline updated:');
  for (const r of rules) console.log(`  ${String(counts[r.id]).padStart(5)}  ${r.id}`);
  process.exit(0);
}

// `--json`: the whole finding set, machine-readable, on stdout — and ALWAYS
// exit 0. The PR reviewer in .github/scripts/ consumes this as evidence, and
// it needs the findings even (especially) on a run that would fail the gate,
// so the exit code has to carry "did the tool run", not "is the tree clean".
// Text mode below is unchanged and stays the thing humans and CI read.
if (process.argv.includes('--json')) {
  process.stdout.write(
    JSON.stringify(
      {
        fileCount,
        rules: rules.map((r) => ({
          id: r.id,
          why: r.why,
          enforce: r.enforce ?? 'baseline',
          baseline: baseline.counts?.[r.id] ?? 0,
          count: findings.get(r.id).length,
          findings: findings.get(r.id),
        })),
      },
      null,
      2,
    ) + '\n',
  );
  process.exit(0);
}

let failed = false;
console.log(`ui-lint: ${fileCount} files, ${rules.length} rules\n`);

for (const rule of rules) {
  if (only && rule.id !== only) continue;
  const found = findings.get(rule.id);
  const base = baseline.counts?.[rule.id] ?? 0;
  const limit = rule.enforce === 'zero' ? 0 : base;
  const over = found.length > limit;
  if (over) failed = true;

  const status = over ? 'FAIL' : found.length < base ? 'improved' : 'ok';
  const budget = rule.enforce === 'zero' ? 'must be 0' : `baseline ${base}`;
  console.log(`[${status.padEnd(8)}] ${rule.id}  ${found.length} (${budget})`);

  if (over || only) {
    console.log(`            ${rule.why}`);
    const show = over ? found.slice(0, 15) : found.slice(0, 30);
    for (const f of show) console.log(`            ${f.file}:${f.line}  ${f.message}`);
    if (found.length > show.length) console.log(`            … and ${found.length - show.length} more`);
  }
  if (found.length < base && !only) {
    console.log(`            ${base - found.length} fewer than baseline — run --update-baseline to lock it in`);
  }
}

if (failed) {
  console.error(
    '\nui-lint failed. A `must be 0` rule was violated, or a ratcheted count went up.\n' +
      'Fix the finding — do not raise the baseline. The baseline exists to stop existing debt\n' +
      'from blocking you, not to absorb new debt.\n',
  );
  process.exit(1);
}
console.log('\nui-lint passed.');
