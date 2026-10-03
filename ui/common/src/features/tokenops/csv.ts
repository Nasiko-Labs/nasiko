/**
 * CSV export (plan F3, A18). Every cell is quoted with inner quotes doubled, and any
 * text cell a spreadsheet would treat as a formula gets a leading apostrophe. The check
 * runs after leading whitespace (some importers trim it) and includes pipe and the
 * full-width variants. Numbers are exported raw, so a negative Δ stays numeric.
 */
import { downloadText } from '@/lib/download'

export interface CsvColumn<T> {
  header: string
  value: (row: T) => string | number | null | undefined
}

const FORMULA = /^\s*[=+\-@\t\r|＝＋－＠]/

export function csvCell(v: string | number | null | undefined): string {
  if (v === null || v === undefined) return '""'
  if (typeof v === 'number') return Number.isFinite(v) ? `"${v}"` : '""'
  const safe = FORMULA.test(v) ? `'${v}` : v
  return `"${safe.replace(/"/g, '""')}"`
}

export function toCsv<T>(columns: CsvColumn<T>[], rows: T[]): string {
  const lines = [columns.map((c) => csvCell(c.header)).join(',')]
  for (const row of rows) lines.push(columns.map((c) => csvCell(c.value(row))).join(','))
  return lines.join('\r\n')
}

export function downloadCsv(filename: string, csv: string): void {
  // The BOM makes Excel read the file as UTF-8.
  downloadText(filename, `\uFEFF${csv}`, 'text/csv;charset=utf-8')
}
