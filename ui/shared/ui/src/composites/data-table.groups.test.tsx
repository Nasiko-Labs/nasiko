/** DataTable's groups (plans/feat-agent-centric.md eng "DataTable gains grouping"): a header row per run, collapsible. */
import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'
import { DataTable, type DataTableColumn } from './data-table'

type R = { id: string; g: string }
const columns: DataTableColumn<R>[] = [
  { id: 'id', header: 'Id', cell: ({ row }) => row.original.id },
]

function Grouped({ data }: { data: R[] }) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set())
  return (
    <DataTable
      label="t"
      columns={columns}
      data={data}
      getRowId={(r) => r.id}
      groups={{
        of: (r) => r.g,
        header: (key) => `Group ${key}`,
        collapsed,
        onToggle: (key) =>
          setCollapsed((prev) => {
            const next = new Set(prev)
            if (!next.delete(key)) next.add(key)
            return next
          }),
      }}
    />
  )
}

const bodyRows = () =>
  screen
    .getAllByRole('row')
    .slice(1)
    .filter((r) => !r.hasAttribute('data-group'))

describe('DataTable groups', () => {
  it('puts a header row before each run, and a toggle collapses only its run', async () => {
    render(
      <Grouped
        data={[
          { id: 'a', g: 'x' },
          { id: 'b', g: 'x' },
          { id: 'c', g: 'y' },
        ]}
      />,
    )
    expect(screen.getAllByRole('row').filter((r) => r.hasAttribute('data-group'))).toHaveLength(2)
    expect(bodyRows()).toHaveLength(3)
    const x = screen.getByRole('button', { name: 'Group x' })
    expect(x).toHaveAttribute('aria-expanded', 'true')
    await userEvent.click(x)
    expect(x).toHaveAttribute('aria-expanded', 'false')
    expect(bodyRows().map((r) => r.textContent)).toEqual(['c'])
    await userEvent.click(x)
    expect(bodyRows()).toHaveLength(3)
  })

  it('counts header rows in aria-rowcount once virtualised', () => {
    const data = Array.from({ length: 120 }, (_, i) => ({ id: `r${i}`, g: i < 60 ? 'x' : 'y' }))
    render(<Grouped data={data} />)
    // 120 rows + 2 header rows + the column header row.
    expect(screen.getByRole('table')).toHaveAttribute('aria-rowcount', '123')
  })
})
