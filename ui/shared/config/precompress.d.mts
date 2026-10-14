/** File names (by extension) that the build writes `.br` / `.gz` variants for. */
export declare const COMPRESSIBLE: RegExp;
/** Write `.br` / `.gz` siblings for the compressible files under `dir`; returns what was written. */
export declare function precompress(dir: string): { br: number; gz: number; skipped: number };
