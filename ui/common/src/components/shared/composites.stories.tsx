// The shared composites (plan §8 Phase 1), one story each. Every story runs as a browser test with axe
// (`npm run test:stories`), in light and dark.
import type { Meta, StoryObj } from '@storybook/react-vite'
import { Bot } from 'lucide-react'
import { useState } from 'react'
import { expect, userEvent, within } from 'storybook/test'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ApiError } from '@/lib/api/client'
import { CopyButton } from './copy-button'
import { DataTable, type DataTableColumn } from './data-table'
import { Delta } from './delta'
import { Disclosure } from './disclosure'
import { KpiTile } from './kpi-tile'
import { PageLoader } from './page-loader'
import { PageHeader } from './page-header'
import { Panel, PanelEmpty, PanelError, PanelSkeleton } from './panel'
import { SearchInput } from './search-input'
import { EmptyState, StateCard } from './state-card'
import { TimeControl } from './time-control'

const meta: Meta = { title: 'Shared/Composites' }
export default meta
type Story = StoryObj

export const Header: Story = {
  render: () => (
    <PageHeader
      title="TokenOps"
      description="What you spent, and what drove it."
      actions={<Button size="sm">Export CSV</Button>}
    />
  ),
}

export const States: Story = {
  render: () => (
    <div className="flex max-w-xl flex-col gap-4">
      <StateCard
        title="Observability isn't configured on this server."
        fix="Set TEMPO_URL and restart it."
      />
      <StateCard
        tone="warning"
        title="You don't have access to this."
        fix="Ask an admin or the owner for access."
      />
      <StateCard
        tone="error"
        title="Can't reach nasiko-server"
        action={
          <Button size="sm" variant="outline">
            Retry
          </Button>
        }
      />
      <EmptyState title="No sessions in this window">
        Pick a longer window, or check that agents export traces.
      </EmptyState>
      <EmptyState
        icon={Bot}
        title="No agents yet"
        action={<Button size="sm">Deploy an agent</Button>}
      >
        Deploy one from a zip, GitHub or a registry and it shows up here.
      </EmptyState>
    </div>
  ),
}

export const Loader: Story = {
  render: () => <PageLoader label="Loading agents" />,
}

export const Panels: Story = {
  render: () => (
    <div className="grid max-w-3xl gap-4 md:grid-cols-2">
      <Panel title="Spend over time" subtitle="Last 30 days" labelledBy="p-spend">
        <PanelSkeleton height={120} />
      </Panel>
      <Panel title="Who is driving cost" labelledBy="p-drivers">
        <PanelEmpty title="No spend in this window" />
      </Panel>
      <Panel title="This month" labelledBy="p-month">
        <PanelError
          error={
            new ApiError(500, 'trace store timeout', '/api/observability/finops/dashboard', 'm')
          }
          onRetry={() => {}}
          what="spend"
        />
      </Panel>
      <Panel title="Harness usage" labelledBy="p-access">
        <PanelError
          error={new ApiError(403, 'requires admin role', '/api/users/me', 'm')}
          onRetry={() => {}}
          what="usage"
        />
      </Panel>
    </div>
  ),
}

export const Kpis: Story = {
  render: () => (
    <div className="grid max-w-3xl grid-cols-3 gap-3">
      <KpiTile
        label="Total spend"
        value="$159.28"
        aside={<Delta changePct={-31.1} polarity="down-good" />}
      />
      <KpiTile label="Tokens" value="4.2M" aside={<Delta changePct={12} polarity="neutral" />} />
      <KpiTile
        label="vs previous"
        value="—"
        aside={<Delta changePct={null} polarity="down-good" unavailable />}
      />
      <div className="col-span-3 flex gap-2">
        <Badge variant="success">Running</Badge>
        <Badge variant="warning">Needs attention</Badge>
        <Badge variant="info">Deploying</Badge>
        <Badge variant="muted">Stopped</Badge>
      </div>
    </div>
  ),
}

function DisclosureDemo() {
  const [open, setOpen] = useState(false)
  return (
    <Disclosure
      id="demo"
      title="All metrics"
      hint="12 figures"
      open={open}
      onToggle={() => setOpen((o) => !o)}
    >
      <p className="py-2 text-sm">Every KPI for the window.</p>
    </Disclosure>
  )
}

export const DisclosureToggle: Story = {
  render: () => <DisclosureDemo />,
  play: async ({ canvasElement }) => {
    const c = within(canvasElement)
    const trigger = c.getByRole('button', { name: /All metrics/ })
    await expect(trigger).toHaveAttribute('aria-expanded', 'false')
    await userEvent.click(trigger)
    await expect(trigger).toHaveAttribute('aria-expanded', 'true')
    await expect(c.getByText('Every KPI for the window.')).toBeVisible()
  },
}

type Row = { id: string; agent: string; cost: number; sessions: number }
const ROWS: Row[] = [
  { id: 'a', agent: 'Code Reviewer', cost: 52.1, sessions: 140 },
  { id: 'b', agent: 'QA Tester', cost: 31.4, sessions: 96 },
  { id: 'c', agent: 'Research Agent', cost: 12.9, sessions: 41 },
]
const COLUMNS: DataTableColumn<Row>[] = [
  { accessorKey: 'agent', header: 'Agent', meta: { rowHeader: true } },
  { accessorKey: 'cost', header: 'Cost', cell: (c) => `$${c.getValue<number>().toFixed(2)}` },
  { accessorKey: 'sessions', header: 'Sessions' },
]

export const Table: Story = {
  render: () => (
    <DataTable columns={COLUMNS} data={ROWS} getRowId={(r) => r.id} label="Cost by agent" />
  ),
}

function ControlsDemo() {
  const [preset, setPreset] = useState<'24h' | '7d' | '30d' | 'mtd' | 'last-month' | 'custom'>(
    '30d',
  )
  return (
    <div className="flex flex-col gap-3">
      <TimeControl preset={preset} today="2026-09-26" onChange={(n) => setPreset(n.preset)} />
      <SearchInput aria-label="Filter agents" placeholder="Filter agents" />
      <span className="text-sm">
        Session id{' '}
        <CopyButton text="5eed0001-0000-4000-8000-000000000001" label="Copy session id" showText />
      </span>
    </div>
  )
}

export const Controls: Story = { render: () => <ControlsDemo /> }
