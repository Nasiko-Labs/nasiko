export type PageMark = number | 'gap-start' | 'gap-end'

export const pageCount = (rows: number, size: number) => Math.max(1, Math.ceil(rows / size))

export function pageOf<T>(rows: T[], page: number, size: number): T[] {
  const at = Math.min(Math.max(0, page), pageCount(rows.length, size) - 1)
  return rows.slice(at * size, (at + 1) * size)
}

export function pageWindow(page: number, pages: number): PageMark[] {
  if (pages <= 7) return Array.from({ length: pages }, (_, i) => i)
  const near = [page - 1, page, page + 1].filter((p) => p > 0 && p < pages - 1)
  const out: PageMark[] = [0]
  if ((near[0] ?? 1) > 1) out.push('gap-start')
  out.push(...near)
  if ((near.at(-1) ?? pages - 2) < pages - 2) out.push('gap-end')
  out.push(pages - 1)
  return out
}
