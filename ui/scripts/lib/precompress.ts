// Build-time compression for the SPA bundle (ported from codex/nasiko-react-foundation's
// shared/config/precompress.mjs). The Rust servers embed the build and serve it through oss/spa, which compresses
// nothing at run time: the bundle carries `<file>.br` and `<file>.gz` beside each text asset and the handler picks one
// by `Accept-Encoding`. Brotli is what browsers send on HTTPS; gzip covers plain HTTP (the OSS compose deployment on
// :8080), where Chrome does not offer `br`.
//
// Rules: only formats that compress (scripts, styles, markup, JSON, SVG, text); never images, fonts or archives, which
// are compressed already. Files under 1 KiB are skipped (headers cost more than they save). A variant is written only
// when it is at least 10 % smaller than the original, so the server never serves a larger body for an encoding.
import { readFileSync, writeFileSync } from 'node:fs'
import { brotliCompressSync, constants, gzipSync } from 'node:zlib'

export const COMPRESSIBLE = /\.(?:js|mjs|css|html|json|svg|txt|xml|webmanifest)$/i
const MIN_BYTES = 1024
const MAX_RATIO = 0.9

/** Write `.br` / `.gz` siblings for each compressible file in `paths`; returns what was written. */
export function precompress(paths: Iterable<string>): { br: number; gz: number; skipped: number } {
  const written = { br: 0, gz: 0, skipped: 0 }
  for (const path of paths) {
    if (!COMPRESSIBLE.test(path)) continue
    const source = readFileSync(path)
    if (source.length < MIN_BYTES) {
      written.skipped++
      continue
    }
    const br = brotliCompressSync(source, {
      params: {
        [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY,
        [constants.BROTLI_PARAM_SIZE_HINT]: source.length,
        [constants.BROTLI_PARAM_MODE]: constants.BROTLI_MODE_TEXT,
      },
    })
    if (br.length <= source.length * MAX_RATIO) {
      writeFileSync(`${path}.br`, br)
      written.br++
    }
    const gz = gzipSync(source, { level: 9 })
    if (gz.length <= source.length * MAX_RATIO) {
      writeFileSync(`${path}.gz`, gz)
      written.gz++
    }
  }
  return written
}
