const HEX = /^#([0-9a-f]{6})$/i

export function isHex(v: unknown): v is string {
  return typeof v === 'string' && HEX.test(v)
}

function parse(hex: string): [number, number, number] {
  const m = HEX.exec(hex)
  if (!m) return [0, 0, 0]
  const n = parseInt(m[1], 16)
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255]
}

/** Flat mix of two hex colours (t = 0 → a, 1 → b). Used for prop detail tints. */
export function mixHex(a: string, b: string, t: number): string {
  const A = parse(a)
  const B = parse(b)
  const c = A.map((v, i) => Math.round(v + (B[i] - v) * t))
  return (
    '#' +
    c
      .map((v) => v.toString(16).padStart(2, '0'))
      .join('')
      .toUpperCase()
  )
}
