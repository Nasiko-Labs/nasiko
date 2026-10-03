export interface SquareColumnLayout {
  /** Number of squares in the column */
  count: number
  /** Top-left Y of each square, bottom square first (relative to bar top at 0) */
  positions: number[]
  /** Quantized column height in pixels */
  columnHeight: number
  squareSize: number
  /** Effective gap between squares (may differ when fit mode redistributes) */
  gap: number
}

export interface SquareColumnInput {
  /** Raw bar length in pixels (baseline − value) */
  barLengthPx: number
  /** Square width/height — typically equals bar width */
  squareSize: number
  /** Gap between stacked squares in pixels */
  gap: number
  /** When true, redistribute gap so column height matches barLengthPx exactly */
  fit?: boolean
}

/** Quantize bar length into a stack of square cells. */
export function computeSquareColumn({
  barLengthPx,
  squareSize,
  gap,
  fit = false,
}: SquareColumnInput): SquareColumnLayout {
  if (barLengthPx <= 0 || squareSize <= 0) {
    return { count: 0, positions: [], columnHeight: 0, squareSize, gap }
  }

  if (fit) {
    const count = Math.max(1, Math.floor((barLengthPx + gap) / (squareSize + gap)))
    const effectiveGap =
      count > 1 ? Math.max(0, (barLengthPx - count * squareSize) / (count - 1)) : 0
    const step = squareSize + effectiveGap
    const columnHeight = barLengthPx
    const positions: number[] = []

    for (let i = 0; i < count; i++) {
      positions.push(columnHeight - squareSize - i * step)
    }

    return {
      count,
      positions,
      columnHeight,
      squareSize,
      gap: effectiveGap,
    }
  }

  const step = squareSize + gap
  const count = Math.max(1, Math.round(barLengthPx / step))
  const columnHeight = count * squareSize + Math.max(0, count - 1) * gap

  const positions: number[] = []
  for (let i = 0; i < count; i++) {
    const offsetFromBottom = i * step
    positions.push(columnHeight - squareSize - offsetFromBottom)
  }

  return { count, positions, columnHeight, squareSize, gap }
}

/** Y center of the topmost square in a vertical column. */
export function topSquareCenterY({
  baselineY,
  barLengthPx,
  squareSize,
  gap,
  fit = false,
}: SquareColumnInput & { baselineY: number }): number {
  const {
    count,
    squareSize: size,
    columnHeight,
  } = computeSquareColumn({
    barLengthPx,
    squareSize,
    gap,
    fit,
  })

  if (count === 0) {
    return baselineY
  }

  const topY = baselineY - columnHeight
  return topY + size / 2
}
