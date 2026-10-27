import type { Scene, SceneDef, SceneNode } from './scene'
import type { Box } from './path'

const esc = (s: string) =>
  s.replace(/[<>&"]/g, (c) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;' })[c] ?? c)
const n2 = (v: number) => (Math.round(v * 100) / 100).toString()

export const nodeMarkup = (n: SceneNode, prefix = 'weave') => {
  if (!n.d) return ''
  const fill = n.grad ? `url(#${prefix}-${n.grad})` : n.fill
  const filter = n.filter ? ` filter="url(#${prefix}-${n.filter})"` : ''
  const clip = n.clip ? ` clip-path="url(#${prefix}-${n.clip})"` : ''
  const op = n.opacity !== undefined && n.opacity < 1 ? ` opacity="${n2(n.opacity)}"` : ''
  const st = n.stroke
    ? ` stroke="${esc(n.stroke.color)}" stroke-width="${n2(n.stroke.width)}" stroke-linejoin="${n.stroke.join ?? 'round'}"${n.stroke.cap ? ` stroke-linecap="${n.stroke.cap}"` : ''}${n.stroke.opacity < 1 ? ` stroke-opacity="${n2(n.stroke.opacity)}"` : ''}`
    : ''
  const tf = n.transform ? ` transform="${n.transform}"` : ''
  return `<path d="${n.d}" fill="${esc(fill)}"${n.evenodd ? ' fill-rule="evenodd"' : ''}${tf}${op}${st}${filter}${clip}/>`
}

const defMarkup = (d: SceneDef, prefix: string) => {
  const id = `${prefix}-${d.id}`
  if (d.kind === 'clip')
    return `<clipPath id="${id}"${d.clipBy ? ` clip-path="url(#${prefix}-${d.clipBy})"` : ''}><path d="${d.d}"${d.evenodd ? ' clip-rule="evenodd"' : ''}/></clipPath>`
  if (d.kind === 'blur')
    return `<filter id="${id}" x="-60%" y="-60%" width="220%" height="220%"><feGaussianBlur stdDeviation="${n2(d.std)}"/></filter>`
  const stops = d.stops
    .map(
      (s) =>
        `<stop offset="${n2(s.offset)}" stop-color="${esc(s.color)}"${s.opacity < 1 ? ` stop-opacity="${n2(s.opacity)}"` : ''}/>`,
    )
    .join('')
  return d.kind === 'linear'
    ? `<linearGradient id="${id}" gradientUnits="userSpaceOnUse" x1="${n2(d.x1)}" y1="${n2(d.y1)}" x2="${n2(d.x2)}" y2="${n2(d.y2)}">${stops}</linearGradient>`
    : `<radialGradient id="${id}" gradientUnits="userSpaceOnUse" cx="${n2(d.cx)}" cy="${n2(d.cy)}" r="${n2(d.r)}">${stops}</radialGradient>`
}

/**
 * Inner markup for one character. Clip paths are prefixed with `prefix`, so
 * copies on one page stay apart and an export is self-contained.
 */
export function sceneMarkup(scene: Scene, prefix = 'weave'): string {
  const defs = scene.defs.length
    ? `<defs>${scene.defs.map((d) => defMarkup(d, prefix)).join('')}</defs>`
    : ''
  const body = scene.nodes.map((n) => nodeMarkup(n, prefix)).join('')
  const props = scene.props
    .map(
      (p) =>
        `<g transform="${p.transform}"${p.opacity < 1 ? ` opacity="${p.opacity.toFixed(3)}"` : ''}>${p.parts.map((n) => nodeMarkup(n, prefix)).join('')}</g>`,
    )
    .join('')
  return `${defs}<g>${body}${props}</g>`
}

export function paddedBounds(scene: Scene, pad = 16): Box {
  const b = scene.bounds
  return { x0: b.x0 - pad, y0: b.y0 - pad, x1: b.x1 + pad, y1: b.y1 + pad }
}

/** Self-contained static SVG of one pose. */
export function sceneToSvg(
  scene: Scene,
  opts: { background?: string | null; title?: string } = {},
): string {
  const b = paddedBounds(scene, 24)
  const w = b.x1 - b.x0
  const h = b.y1 - b.y0
  const vb = `${b.x0.toFixed(1)} ${b.y0.toFixed(1)} ${w.toFixed(1)} ${h.toFixed(1)}`
  const bg = opts.background
    ? `<rect x="${b.x0.toFixed(1)}" y="${b.y0.toFixed(1)}" width="${w.toFixed(1)}" height="${h.toFixed(1)}" fill="${esc(opts.background)}"/>`
    : ''
  const title = opts.title ? `<title>${esc(opts.title)}</title>` : ''
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${vb}" width="${Math.round(w)}" height="${Math.round(h)}">${title}${bg}${sceneMarkup(scene)}</svg>\n`
}
