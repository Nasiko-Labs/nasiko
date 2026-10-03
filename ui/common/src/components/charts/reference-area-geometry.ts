export type ReferenceAreaIfOverflow = 'hidden' | 'visible' | 'discard'

export interface ReferenceAreaRect {
  x: number
  y: number
  width: number
  height: number
}

export interface ComputeReferenceAreaRectOptions {
  innerWidth: number
  innerHeight: number
  x1?: Date | number
  x2?: Date | number
  y1?: number
  y2?: number
  ifOverflow?: ReferenceAreaIfOverflow
  xScale: (value: Date) => number
  yScale: (value: number) => number
}

function toDate(value: Date | number): Date {
  return value instanceof Date ? value : new Date(value)
}

function resolveXPixel(
  xScale: (value: Date) => number,
  value: Date | number | undefined,
  fallback: number,
): number {
  if (value == null) {
    return fallback
  }
  return xScale(toDate(value))
}

function resolveYPixel(
  yScale: (value: number) => number,
  value: number | undefined,
  fallback: number,
): number {
  if (value == null) {
    return fallback
  }
  return yScale(value)
}

function clampRectToPlot(
  rect: ReferenceAreaRect,
  innerWidth: number,
  innerHeight: number,
): ReferenceAreaRect | null {
  const x1 = Math.max(0, rect.x)
  const y1 = Math.max(0, rect.y)
  const x2 = Math.min(innerWidth, rect.x + rect.width)
  const y2 = Math.min(innerHeight, rect.y + rect.height)
  const width = x2 - x1
  const height = y2 - y1
  if (width <= 0 || height <= 0) {
    return null
  }
  return { x: x1, y: y1, width, height }
}

function isFullyInsidePlot(
  rect: ReferenceAreaRect,
  innerWidth: number,
  innerHeight: number,
): boolean {
  return (
    rect.x >= 0 &&
    rect.y >= 0 &&
    rect.x + rect.width <= innerWidth &&
    rect.y + rect.height <= innerHeight
  )
}

/** Map data bounds to plot pixels for a reference rectangle. */
export function computeReferenceAreaRect(
  options: ComputeReferenceAreaRectOptions,
): ReferenceAreaRect | null {
  const { innerWidth, innerHeight, x1, x2, y1, y2, ifOverflow = 'hidden', xScale, yScale } = options

  if (innerWidth <= 0 || innerHeight <= 0) {
    return null
  }

  const left = resolveXPixel(xScale, x1, 0)
  const right = resolveXPixel(xScale, x2, innerWidth)
  const top = resolveYPixel(yScale, y1, 0)
  const bottom = resolveYPixel(yScale, y2, innerHeight)

  const x = Math.min(left, right)
  const y = Math.min(top, bottom)
  const width = Math.abs(right - left)
  const height = Math.abs(bottom - top)

  if (width <= 0 || height <= 0) {
    return null
  }

  const rect: ReferenceAreaRect = { x, y, width, height }

  if (ifOverflow === 'visible') {
    return rect
  }

  if (ifOverflow === 'discard') {
    return isFullyInsidePlot(rect, innerWidth, innerHeight) ? rect : null
  }

  return clampRectToPlot(rect, innerWidth, innerHeight)
}

/** Inclusive data range for axis label highlighting. */
export function resolveReferenceDataRange(
  y1: number | undefined,
  y2: number | undefined,
  domain: [number, number],
): [number, number] {
  const dMin = Math.min(domain[0], domain[1])
  const dMax = Math.max(domain[0], domain[1])
  const low = y1 ?? dMin
  const high = y2 ?? dMax
  return [Math.min(low, high), Math.max(low, high)]
}
