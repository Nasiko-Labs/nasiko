import { PageHeader } from '@/components/shared/page-header'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { rows } from './eval'

type Row = (typeof rows)[number]
type RequestRow = Extract<Row, { baseline_tokens: number }>
type DecoderRow = Extract<Row, { decoded: unknown }>

function isDecoder(row: Row): row is DecoderRow {
  return 'decoded' in row
}

function toolLabel(row: Row) {
  return row.tool_names.length ? row.tool_names.join(' + ') : 'No tool'
}

function resultOf(row: Row): {
  text: string
  variant: 'success' | 'warning' | 'destructive'
} {
  if (isDecoder(row)) {
    if (row.decoder_ok && row.expected_error) {
      return { text: 'PASS — rejected correctly', variant: 'warning' }
    }
    if (row.decoder_ok) return { text: 'PASS', variant: 'success' }
    return { text: 'FAIL', variant: 'destructive' }
  }
  return row.roundtrip_success
    ? { text: 'PASS', variant: 'success' }
    : { text: 'FAIL', variant: 'destructive' }
}

const num = new Intl.NumberFormat('en-US')
const pct = (n: number) => `${n.toFixed(1)}%`

export function CompactToolsPage() {
  const measured = rows.filter((row): row is RequestRow => !isDecoder(row))
  const baseline = measured.reduce((sum, row) => sum + row.baseline_tokens, 0)
  const compact = measured.reduce((sum, row) => sum + row.compact_tokens, 0)
  const saved = baseline - compact
  const reduction = baseline ? (saved / baseline) * 100 : 0
  const passed = rows.filter((row) => resultOf(row).text.startsWith('PASS')).length
  const max = Math.max(baseline, compact, 1)

  const tools = new Map<string, { baseline: number; compact: number }>()
  for (const row of measured) {
    const key = toolLabel(row)
    const group = tools.get(key) ?? { baseline: 0, compact: 0 }
    group.baseline += row.baseline_tokens
    group.compact += row.compact_tokens
    tools.set(key, group)
  }

  const requests = rows.filter((row): row is RequestRow => !isDecoder(row))
  const requestOk = requests.filter((row) => row.roundtrip_success).length
  const stream = rows.filter((row): row is DecoderRow => isDecoder(row) && !row.expected_error)
  const streamOk = stream.filter((row) => row.decoder_ok).length
  const rejected = rows.filter((row): row is DecoderRow => isDecoder(row) && row.expected_error)
  const rejectedOk = rejected.filter((row) => row.decoder_ok).length

  const kpis: { label: string; value: string; hero?: boolean }[] = [
    { label: 'Total cases', value: num.format(rows.length) },
    { label: 'Baseline tokens', value: num.format(baseline) },
    { label: 'Compact tokens', value: num.format(compact) },
    { label: 'Tokens saved', value: num.format(saved) },
    { label: 'Token reduction', value: pct(reduction), hero: true },
    { label: 'Success rate', value: pct(rows.length ? (passed / rows.length) * 100 : 0) },
  ]

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Compact tool schemas"
        description="Compact schemas cut prompt tokens and keep the tool calls correct."
      />

      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-6">
        {kpis.map((kpi) => (
          <Card key={kpi.label} className="gap-1 px-4 py-4">
            <div className="text-xs text-muted-foreground">{kpi.label}</div>
            <div
              className={
                kpi.hero ? 'text-2xl font-semibold text-success' : 'text-2xl font-semibold'
              }
            >
              {kpi.value}
            </div>
          </Card>
        ))}
      </div>

      <Card className="gap-3 px-4 py-4">
        <div className="text-sm font-medium">Baseline vs compact</div>
        <Bar name="Baseline" value={baseline} max={max} className="bg-primary" />
        <Bar name="Compact" value={compact} max={max} className="bg-success" />
        <p className="text-xs text-muted-foreground">
          Aggregate reduction is (baseline − compact) / baseline, using o200k_base.
          Decoder cases are not included in the token total.
        </p>
      </Card>

      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">Tool token usage</h2>
        <Card className="gap-0 py-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Tool</TableHead>
                <TableHead>Baseline tokens</TableHead>
                <TableHead>Compact tokens</TableHead>
                <TableHead>Tokens saved</TableHead>
                <TableHead>Reduction</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {[...tools.entries()].map(([name, group]) => {
                const toolSaved = group.baseline - group.compact
                const toolReduction = group.baseline ? (toolSaved / group.baseline) * 100 : 0
                return (
                  <TableRow key={name}>
                    <TableCell className="font-mono text-xs">{name}</TableCell>
                    <TableCell>{num.format(group.baseline)}</TableCell>
                    <TableCell>{num.format(group.compact)}</TableCell>
                    <TableCell>{num.format(toolSaved)}</TableCell>
                    <TableCell>{pct(toolReduction)}</TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        </Card>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">Cases</h2>
        <Card className="gap-0 py-0">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Case</TableHead>
                <TableHead>Tool</TableHead>
                <TableHead>Baseline</TableHead>
                <TableHead>Compact</TableHead>
                <TableHead>Saved</TableHead>
                <TableHead>Reduction</TableHead>
                <TableHead>Result</TableHead>
                <TableHead>Call</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((row) => {
                const result = resultOf(row)
                const call = isDecoder(row)
                  ? JSON.stringify(row.decoded.calls ?? row.decoded)
                  : row.rendered_calls || '—'
                return (
                  <TableRow key={row.id}>
                    <TableCell className="font-mono text-xs">{row.id}</TableCell>
                    <TableCell className="font-mono text-xs">{toolLabel(row)}</TableCell>
                    <TableCell>{isDecoder(row) ? '—' : num.format(row.baseline_tokens)}</TableCell>
                    <TableCell>{isDecoder(row) ? '—' : num.format(row.compact_tokens)}</TableCell>
                    <TableCell>{isDecoder(row) ? '—' : num.format(row.tokens_saved)}</TableCell>
                    <TableCell>{isDecoder(row) ? '—' : pct(row.reduction_percent)}</TableCell>
                    <TableCell>
                      <Badge variant={result.variant}>{result.text}</Badge>
                    </TableCell>
                    <TableCell className="max-w-sm font-mono text-xs whitespace-normal">
                      {call}
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        </Card>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-medium">Tool call correctness</h2>
        <div className="grid gap-3 sm:grid-cols-2">
          <Check label="Valid tool calls" ok={requestOk} total={requests.length} />
          <Check label="Round-trip successful" ok={requestOk} total={requests.length} />
          <Check label="Streaming decoder successful" ok={streamOk} total={stream.length} />
          <Check label="Invalid calls rejected" ok={rejectedOk} total={rejected.length} />
        </div>
      </section>

      <Card className="gap-1 px-4 py-4">
        <div className="text-xs text-muted-foreground">Offline evaluation</div>
        <p className="text-sm">
          Numbers are from <span className="font-mono text-xs">compact_tools_eval</span> (
          <span className="font-mono text-xs">/tmp/compact-tools-eval.json</span>
          ). No live model output is in this run.
        </p>
      </Card>
    </div>
  )
}

function Bar({
  name,
  value,
  max,
  className,
}: {
  name: string
  value: number
  max: number
  className: string
}) {
  return (
    <div className="grid grid-cols-[5.5rem_1fr_auto] items-center gap-3 text-sm">
      <div>{name}</div>
      <div className="h-3 overflow-hidden rounded-full bg-muted">
        <div
          className={`h-3 rounded-full ${className}`}
          style={{ width: `${(value / max) * 100}%` }}
        />
      </div>
      <div className="font-mono text-xs">{num.format(value)}</div>
    </div>
  )
}

function Check({ label, ok, total }: { label: string; ok: number; total: number }) {
  return (
    <Card className="gap-1 px-4 py-3">
      <div className="text-sm font-medium">
        {total === 0 ? '—' : ok === total ? '✓' : '✗'} {label}
      </div>
      <div className="text-xs text-muted-foreground">
        {total === 0 ? 'Not in this run' : `${ok} / ${total}`}
      </div>
    </Card>
  )
}
