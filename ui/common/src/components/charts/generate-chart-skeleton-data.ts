const DEFAULT_SKELETON_DATA_KEY = 'value'
const DEFAULT_SKELETON_POINT_COUNT = 7

export interface GenerateChartSkeletonDataOptions {
  /** Key used for y values in each row. Default: `"value"`. */
  dataKey?: string
  /** Number of points. Default: 7. */
  pointCount?: number
  /** Start date for the x axis. Default: 2025-01-01. */
  baseDate?: Date
}

/** Placeholder series used while `status="loading"` and data is empty. */
export function generateChartSkeletonData(
  options: GenerateChartSkeletonDataOptions = {},
): Record<string, unknown>[] {
  const dataKey = options.dataKey ?? DEFAULT_SKELETON_DATA_KEY
  const pointCount = options.pointCount ?? DEFAULT_SKELETON_POINT_COUNT
  const baseDate = options.baseDate ?? new Date('2025-01-01')

  return Array.from({ length: pointCount }, (_, index) => {
    const date = new Date(baseDate)
    date.setDate(baseDate.getDate() + index)
    return {
      date,
      [dataKey]: Math.round(110 + Math.sin(index * 1.15) * 36 + index * 9),
    }
  })
}

/** Skeleton rows that mirror target dates/count with lower magnitudes for Y tween. */
export function generateChartSkeletonFromTarget(
  targetData: Record<string, unknown>[],
  dataKey: string,
): Record<string, unknown>[] {
  return targetData.map((row, index) => ({
    ...row,
    [dataKey]: Math.round(95 + Math.sin(index * 1.05) * 28 + index * 7),
  }))
}

export { DEFAULT_SKELETON_DATA_KEY, DEFAULT_SKELETON_POINT_COUNT }
