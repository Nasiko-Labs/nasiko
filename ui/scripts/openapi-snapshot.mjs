#!/usr/bin/env node
/**
 * Snapshot the control plane's OpenAPI spec into `ui/contracts/openapi.snapshot.json`,
 * and raise an alarm when the committed copy no longer matches a running server.
 *
 * ## Why a snapshot, and why a second half
 *
 * `gen-data-manifest.mjs` needs the backend's response shapes to check the
 * shapes the data manifest declares. The only machine-readable source of those
 * is the utoipa spec the server serves at `/api/ee/openapi.json` — which means
 * a running server. CI has no server, and a generator that needs the network
 * is a generator that fails differently on every machine. So the spec is
 * committed, and the generator reads the committed copy: deterministic, and
 * a backend shape change arrives as a readable diff in review instead of as a
 * surprise on someone's laptop.
 *
 * A committed copy goes stale silently, though. That is the trade: flaky-but-
 * fresh for stable-and-quietly-wrong. `--check` is the other half — run it
 * where a server actually exists (a dev task, a scheduled job, `just
 * openapi-check` after `just dev`) and it fails loudly the moment the live
 * spec and the committed one disagree. Snapshot for determinism, alarm for
 * staleness. Neither is sufficient alone.
 *
 * ## Canonical form
 *
 * Keys are sorted recursively and the file is pretty-printed with two-space
 * indentation. utoipa's own serialization order is not stable across code
 * moves, and a snapshot whose serialization churns rewrites the whole file on
 * every regeneration — which loses the one thing that makes it worth
 * committing: a shape change showing up as a small diff. Arrays keep their
 * order; in OpenAPI, order inside `required`, `enum` and `tags` is meaning.
 *
 * ## What this file is, and is not, an input to
 *
 * It feeds `gen-data-manifest.mjs --check` only. It is not copied into the
 * Weave image, it is not in Weave's `_GENERATOR_SOURCES`, and committing a new
 * snapshot does not move `generatorDigest` on its own — only a regenerated
 * data-manifest.json does. It lives under `ui/contracts/`, not
 * `ui/common/surface/`, because everything under `common/` is embedded into
 * the server binary and served, and a 300 KB spec the browser never reads has
 * no business there.
 *
 * Usage:
 *   node ui/scripts/openapi-snapshot.mjs --write [--url URL]
 *   node ui/scripts/openapi-snapshot.mjs --check [--url URL]
 *   node ui/scripts/openapi-snapshot.mjs --canon FILE      # canonicalize a file you fetched some other way
 *
 * URL defaults to $NASIKO_OPENAPI_URL, then http://localhost:8082/api/ee/openapi.json.
 * The EE mount is the right one: it is the OSS spec with the EE paths merged
 * in (the EE OpenAPI module), so it is a superset of /api/openapi.json.
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const SNAPSHOT = resolve(UI, 'contracts/openapi.snapshot.json');
const DEFAULT_URL = process.env.NASIKO_OPENAPI_URL || 'http://localhost:8082/api/ee/openapi.json';

const fail = (msg, hint) => {
  console.error(`openapi-snapshot: ${msg}`);
  if (hint) console.error(`  ${hint}`);
  process.exit(1);
};

/** Recursively sort object keys. Arrays keep their order — see the header. */
export function sortKeys(v) {
  if (Array.isArray(v)) return v.map(sortKeys);
  if (v && typeof v === 'object') {
    return Object.fromEntries(Object.keys(v).sort().map((k) => [k, sortKeys(v[k])]));
  }
  return v;
}

export function canonical(spec) {
  return `${JSON.stringify(sortKeys(spec), null, 2)}\n`;
}

async function fetchLive(url) {
  let res;
  try {
    res = await fetch(url);
  } catch (e) {
    fail(`could not reach ${url}: ${e.message}`,
      'This check needs a running control plane. Start one (`just dev`) or point --url / NASIKO_OPENAPI_URL at one.');
  }
  if (!res.ok) fail(`${url} answered ${res.status}`);
  return res.json();
}

/**
 * A summary a reviewer can read, not a 300 KB diff. Paths and schemas that
 * appeared, disappeared, or changed — enough to decide whether the drift is
 * "someone annotated a new route" or "a shape the manifest declares moved".
 */
export function summarizeDrift(committed, live) {
  const lines = [];
  const setDiff = (label, a, b) => {
    const A = new Set(Object.keys(a ?? {}));
    const B = new Set(Object.keys(b ?? {}));
    for (const k of B) if (!A.has(k)) lines.push(`  + ${label} ${k}`);
    for (const k of A) if (!B.has(k)) lines.push(`  - ${label} ${k}`);
    for (const k of A) {
      if (B.has(k) && JSON.stringify(sortKeys(a[k])) !== JSON.stringify(sortKeys(b[k]))) {
        lines.push(`  ~ ${label} ${k}`);
      }
    }
  };
  setDiff('path', committed.paths, live.paths);
  setDiff('schema', committed.components?.schemas, live.components?.schemas);
  return lines;
}

const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const opt = (name) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : undefined; };

if (import.meta.url === `file://${process.argv[1]}`) {
  const url = opt('--url') || DEFAULT_URL;

  if (flag('--canon')) {
    const file = opt('--canon');
    const text = canonical(JSON.parse(readFileSync(file, 'utf8')));
    mkdirSync(dirname(SNAPSHOT), { recursive: true });
    writeFileSync(SNAPSHOT, text);
    console.log(`openapi-snapshot: wrote ${SNAPSHOT} from ${file}`);
  } else if (flag('--write')) {
    const live = await fetchLive(url);
    mkdirSync(dirname(SNAPSHOT), { recursive: true });
    writeFileSync(SNAPSHOT, canonical(live));
    console.log(`openapi-snapshot: wrote ${SNAPSHOT} from ${url} `
      + `(${Object.keys(live.paths ?? {}).length} paths, ${Object.keys(live.components?.schemas ?? {}).length} schemas)`);
  } else if (flag('--check')) {
    if (!existsSync(SNAPSHOT)) fail('no committed snapshot — run with --write first.');
    const committedText = readFileSync(SNAPSHOT, 'utf8');
    const live = await fetchLive(url);
    const liveText = canonical(live);
    if (committedText !== liveText) {
      const drift = summarizeDrift(JSON.parse(committedText), live);
      console.error(`openapi-snapshot: ${SNAPSHOT} has drifted from ${url}:`);
      for (const l of drift) console.error(l);
      if (!drift.length) console.error('  (info or top-level metadata changed)');
      fail('the committed spec no longer matches the running server.',
        'Run: node ui/scripts/openapi-snapshot.mjs --write, then node ui/scripts/gen-data-manifest.mjs --check '
        + 'to find out which declared shapes the change touches.');
    }
    console.log(`openapi-snapshot: snapshot matches ${url}`);
  } else {
    fail('pass --write, --check or --canon FILE');
  }
}
