// Build-time compression for the SPA bundle. The Rust control plane embeds the build and serves it (oss/server/src/
// spa.rs); it compresses nothing at run time, so the bundle carries `<file>.br` and `<file>.gz` beside each text asset
// and spa.rs picks one by `Accept-Encoding`. Brotli is what browsers send on HTTPS; gzip covers plain HTTP (the OSS
// compose deployment on :8080), where Chrome does not offer `br`.
//
// Rules: only formats that compress (scripts, styles, markup, JSON, SVG, text); never images, fonts or archives, which
// are compressed already. Files under 1 KiB are skipped (headers cost more than they save). A variant is written only
// when it is at least 10 % smaller than the original, so spa.rs never serves a larger body for an encoding.
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { brotliCompressSync, constants, gzipSync } from "node:zlib";

export const COMPRESSIBLE = /\.(?:js|mjs|css|html|json|svg|txt|xml|webmanifest|map)$/i;
const MIN_BYTES = 1024;
const MIN_SAVING = 0.9;

function* files(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) yield* files(path);
    else yield path;
  }
}

/** Write `.br` / `.gz` siblings for the compressible files under `dir`; returns what was written. */
export function precompress(dir) {
  const written = { br: 0, gz: 0, skipped: 0 };
  for (const path of files(dir)) {
    if (!COMPRESSIBLE.test(path)) continue;
    const source = readFileSync(path);
    if (source.length < MIN_BYTES) {
      written.skipped++;
      continue;
    }
    const br = brotliCompressSync(source, {
      params: {
        [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY,
        [constants.BROTLI_PARAM_SIZE_HINT]: source.length,
        [constants.BROTLI_PARAM_MODE]: constants.BROTLI_MODE_TEXT,
      },
    });
    if (br.length <= source.length * MIN_SAVING) {
      writeFileSync(path + ".br", br);
      written.br++;
    }
    const gz = gzipSync(source, { level: 9 });
    if (gz.length <= source.length * MIN_SAVING) {
      writeFileSync(path + ".gz", gz);
      written.gz++;
    }
  }
  return written;
}
