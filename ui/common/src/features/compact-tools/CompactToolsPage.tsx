import { useState } from 'react'
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  Code2,
  Cpu,
  Shrink,
  Terminal,
  Zap,
} from 'lucide-react'
import { CopyButton } from '@/components/shared/copy-button'
import { KpiTile } from '@/components/shared/kpi-tile'
import { PageHeader } from '@/components/shared/page-header'
import { Panel } from '@/components/shared/panel'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'

const CALENDAR_NATIVE = `{
  "name": "create_calendar_event",
  "description": "Create a new calendar meeting or event",
  "parameters": {
    "type": "object",
    "properties": {
      "title": {
        "type": "string",
        "description": "Event title"
      },
      "start": {
        "type": "string",
        "format": "date-time",
        "description": "Start timestamp in ISO 8601"
      },
      "duration_min": {
        "type": "integer",
        "description": "Duration in minutes"
      },
      "attendees": {
        "type": "array",
        "items": { "type": "string" },
        "description": "List of attendee emails"
      },
      "visibility": {
        "type": "string",
        "enum": ["public", "private"],
        "description": "Event visibility"
      }
    },
    "required": ["title", "start"]
  }
}`

const CALENDAR_COMPACT = `create_calendar_event(
  title:str,
  start:datetime,
  duration_min?:int,
  attendees?:[str],
  visibility?:public|private
)`

const EMAIL_NATIVE = `{
  "name": "send_email",
  "description": "Send an email message to recipients",
  "parameters": {
    "type": "object",
    "properties": {
      "to": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Recipient email addresses"
      },
      "subject": {
        "type": "string",
        "description": "Email subject line"
      },
      "body": {
        "type": "string",
        "description": "Email body content"
      },
      "cc": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Carbon-copy recipient emails"
      }
    },
    "required": ["to", "subject", "body"]
  }
}`

const EMAIL_COMPACT = `send_email(
  to:[str],
  subject:str,
  body:str,
  cc?:[str]
)`

const RAW_MODEL_OUTPUT = `<<call create_calendar_event {
  "title": "Team Meeting",
  "start": "2026-10-03T15:00:00",
  "duration_min": 30,
  "visibility": "private"
}>>`

const DECODED_ARGUMENTS = `{
  "title": "Team Meeting",
  "start": "2026-10-03T15:00:00",
  "duration_min": 30,
  "visibility": "private"
}`

const EVAL_CASES = [
  {
    id: 'ct-001',
    description: 'Single calendar tool',
    nativeTokens: 262,
    compactTokens: 124,
    saved: 138,
    reduction: '52.67%',
  },
  {
    id: 'ct-002',
    description: 'Email + calendar',
    nativeTokens: 271,
    compactTokens: 133,
    saved: 138,
    reduction: '50.92%',
  },
  {
    id: 'ct-003',
    description: 'No tool call',
    nativeTokens: 150,
    compactTokens: 87,
    saved: 63,
    reduction: '42.00%',
  },
]

const VALIDATION_CHECKS = [
  {
    title: 'Unknown tool → rejected',
    desc: 'Calls referencing undeclared or unpermitted tools fail closed immediately.',
  },
  {
    title: 'Missing required argument → rejected',
    desc: 'Calls lacking mandatory schema arguments fail schema validation before execution.',
  },
  {
    title: 'Invalid enum → rejected',
    desc: 'Arguments with values outside declared enum variants fail validation.',
  },
  {
    title: '>> inside JSON string → handled correctly',
    desc: 'Escaped or literal delimiter tokens within JSON string values do not cause early closing.',
  },
  {
    title: 'Multiple tool calls → ordered correctly',
    desc: 'Sequentially streamed tool calls in a turn are parsed and preserved in exact order.',
  },
  {
    title: 'Streaming marker split → handled',
    desc: 'Token chunks splitting across <<call or >> boundary markers are properly buffered.',
  },
  {
    title: 'Unsupported schema features → bypass compaction',
    desc: 'Complex schema constructs (e.g. anyOf, oneOf) safely bypass compaction to native representation.',
  },
]

export function CompactToolsPage() {
  const [calendarTab, setCalendarTab] = useState<'compact' | 'native'>('compact')
  const [emailTab, setEmailTab] = useState<'compact' | 'native'>('compact')

  return (
    <div className="flex flex-col gap-6">
      {/* Header */}
      <PageHeader
        title="Compact Tool Schemas"
        description="Reduce tool-definition token overhead while preserving schema semantics and decoding tool calls back to standard ToolCall format."
        actions={
          <>
            <Badge variant="secondary">P1 · Compact Tool Schemas</Badge>
            <Badge variant="muted">Demo / Evaluation View</Badge>
          </>
        }
      />

      {/* 1. Main Visual Flow */}
      <Panel
        title="Processing Pipeline"
        subtitle="End-to-end transformation from OpenAI-compatible schemas through compact prompt injection and deterministic decoding."
      >
        <div className="pt-1">
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6">
            <div className="flex flex-col items-center justify-between rounded-md border border-border bg-muted/40 p-3 text-center">
              <span className="font-mono text-2xs text-muted-foreground">01</span>
              <div className="my-2 flex size-8 items-center justify-center rounded-md border border-border bg-background">
                <Code2 className="size-4 text-primary" />
              </div>
              <div className="space-y-0.5">
                <p className="text-xs font-semibold">Native Tool Schema</p>
                <p className="text-2xs text-muted-foreground">Standard JSON Schema</p>
              </div>
            </div>

            <div className="flex flex-col items-center justify-between rounded-md border border-border bg-muted/40 p-3 text-center">
              <span className="font-mono text-2xs text-muted-foreground">02</span>
              <div className="my-2 flex size-8 items-center justify-center rounded-md border border-border bg-background">
                <Shrink className="size-4 text-primary" />
              </div>
              <div className="space-y-0.5">
                <p className="text-xs font-semibold">Compact DSL</p>
                <p className="text-2xs text-muted-foreground">Compaction engine</p>
              </div>
            </div>

            <div className="flex flex-col items-center justify-between rounded-md border border-border bg-muted/40 p-3 text-center">
              <span className="font-mono text-2xs text-muted-foreground">03</span>
              <div className="my-2 flex size-8 items-center justify-center rounded-md border border-border bg-background">
                <Cpu className="size-4 text-primary" />
              </div>
              <div className="space-y-0.5">
                <p className="text-xs font-semibold">LLM</p>
                <p className="text-2xs text-muted-foreground">Model inference</p>
              </div>
            </div>

            <div className="flex flex-col items-center justify-between rounded-md border border-border bg-muted/40 p-3 text-center">
              <span className="font-mono text-2xs text-muted-foreground">04</span>
              <div className="my-2 flex size-8 items-center justify-center rounded-md border border-border bg-background">
                <Terminal className="size-4 text-primary" />
              </div>
              <div className="space-y-0.5">
                <p className="font-mono text-xs font-semibold">&lt;&lt;call ...&gt;&gt;</p>
                <p className="text-2xs text-muted-foreground">Delimited syntax</p>
              </div>
            </div>

            <div className="flex flex-col items-center justify-between rounded-md border border-border bg-muted/40 p-3 text-center">
              <span className="font-mono text-2xs text-muted-foreground">05</span>
              <div className="my-2 flex size-8 items-center justify-center rounded-md border border-border bg-background">
                <Zap className="size-4 text-primary" />
              </div>
              <div className="space-y-0.5">
                <p className="text-xs font-semibold">ToolCall Decoder</p>
                <p className="text-2xs text-muted-foreground">Parser & validation</p>
              </div>
            </div>

            <div className="flex flex-col items-center justify-between rounded-md border border-border bg-muted/40 p-3 text-center">
              <span className="font-mono text-2xs text-muted-foreground">06</span>
              <div className="my-2 flex size-8 items-center justify-center rounded-md border border-border bg-background">
                <CheckCircle2 className="size-4 text-success" />
              </div>
              <div className="space-y-0.5">
                <p className="text-xs font-semibold">Standard ToolCall</p>
                <p className="text-2xs text-muted-foreground">OpenAI-compatible</p>
              </div>
            </div>
          </div>
        </div>
      </Panel>

      {/* 2. Token Reduction Key Metrics */}
      <section className="space-y-3" aria-labelledby="token-reduction-h">
        <div className="flex flex-wrap items-baseline justify-between gap-2">
          <div>
            <h2 id="token-reduction-h" className="text-sm font-semibold">
              Token Reduction
            </h2>
            <p className="text-xs text-muted-foreground">
              Measured with <code className="font-mono font-medium">o200k_base</code> on the public
              P1 evaluation dataset.
            </p>
          </div>
          <Badge variant="success">Verified Public Benchmark</Badge>
        </div>

        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <KpiTile
            label="Tool definitions alone"
            value="72.60%"
            aside={<Badge variant="success">-416 tokens</Badge>}
          >
            <div className="mt-2 space-y-1 text-xs text-muted-foreground">
              <div className="flex justify-between border-t border-border pt-1.5">
                <span>Native schema</span>
                <span className="font-mono font-medium tabular-nums text-foreground">
                  573 tokens
                </span>
              </div>
              <div className="flex justify-between">
                <span>Compact DSL</span>
                <span className="font-mono font-medium tabular-nums text-foreground">
                  157 tokens
                </span>
              </div>
              <div className="flex justify-between">
                <span>Tokens saved</span>
                <span className="font-mono font-medium tabular-nums text-success">416 tokens</span>
              </div>
            </div>
          </KpiTile>

          <KpiTile
            label="Full requests"
            value="49.63%"
            aside={<Badge variant="success">-339 tokens</Badge>}
          >
            <div className="mt-2 space-y-1 text-xs text-muted-foreground">
              <div className="flex justify-between border-t border-border pt-1.5">
                <span>Native request</span>
                <span className="font-mono font-medium tabular-nums text-foreground">
                  683 tokens
                </span>
              </div>
              <div className="flex justify-between">
                <span>Compact request</span>
                <span className="font-mono font-medium tabular-nums text-foreground">
                  344 tokens
                </span>
              </div>
              <div className="flex justify-between">
                <span>Tokens saved</span>
                <span className="font-mono font-medium tabular-nums text-success">339 tokens</span>
              </div>
            </div>
          </KpiTile>
        </div>
      </section>

      {/* 3. Schema Compression */}
      <section className="space-y-3" aria-labelledby="schema-compression-h">
        <div>
          <h2 id="schema-compression-h" className="text-sm font-semibold">
            Schema Compression
          </h2>
          <p className="text-xs text-muted-foreground">
            Compact DSL grammar strips JSON Schema boilerplate while preserving strict types,
            defaults, optionals, and enum constraints.
          </p>
        </div>

        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          {/* Tool Card 1: create_calendar_event */}
          <Card className="gap-3 p-4">
            <CardHeader className="flex flex-row items-center justify-between gap-2 p-0">
              <div className="space-y-0.5">
                <CardTitle className="font-mono text-sm">create_calendar_event</CardTitle>
                <CardDescription className="text-xs">
                  5 properties · 2 required (title, start)
                </CardDescription>
              </div>
              <Tabs
                value={calendarTab}
                onValueChange={(v) => setCalendarTab(v as 'compact' | 'native')}
              >
                <TabsList className="h-7">
                  <TabsTrigger value="compact" className="px-2 py-0.5 text-xs">
                    Compact DSL
                  </TabsTrigger>
                  <TabsTrigger value="native" className="px-2 py-0.5 text-xs">
                    Native Schema
                  </TabsTrigger>
                </TabsList>
              </Tabs>
            </CardHeader>

            <CardContent className="p-0">
              {calendarTab === 'compact' ? (
                <div className="relative">
                  <div className="absolute top-2 right-2">
                    <CopyButton text={CALENDAR_COMPACT} label="Copy Compact DSL" />
                  </div>
                  <pre className="overflow-x-auto rounded-md border border-border bg-muted/50 p-3 font-mono text-xs leading-relaxed text-foreground">
                    {CALENDAR_COMPACT}
                  </pre>
                </div>
              ) : (
                <div className="relative">
                  <div className="absolute top-2 right-2">
                    <CopyButton text={CALENDAR_NATIVE} label="Copy Native Schema" />
                  </div>
                  <pre className="max-h-64 overflow-auto rounded-md border border-border bg-muted/50 p-3 font-mono text-2xs leading-relaxed text-foreground">
                    {CALENDAR_NATIVE}
                  </pre>
                </div>
              )}
            </CardContent>
          </Card>

          {/* Tool Card 2: send_email */}
          <Card className="gap-3 p-4">
            <CardHeader className="flex flex-row items-center justify-between gap-2 p-0">
              <div className="space-y-0.5">
                <CardTitle className="font-mono text-sm">send_email</CardTitle>
                <CardDescription className="text-xs">
                  4 properties · 3 required (to, subject, body)
                </CardDescription>
              </div>
              <Tabs value={emailTab} onValueChange={(v) => setEmailTab(v as 'compact' | 'native')}>
                <TabsList className="h-7">
                  <TabsTrigger value="compact" className="px-2 py-0.5 text-xs">
                    Compact DSL
                  </TabsTrigger>
                  <TabsTrigger value="native" className="px-2 py-0.5 text-xs">
                    Native Schema
                  </TabsTrigger>
                </TabsList>
              </Tabs>
            </CardHeader>

            <CardContent className="p-0">
              {emailTab === 'compact' ? (
                <div className="relative">
                  <div className="absolute top-2 right-2">
                    <CopyButton text={EMAIL_COMPACT} label="Copy Compact DSL" />
                  </div>
                  <pre className="overflow-x-auto rounded-md border border-border bg-muted/50 p-3 font-mono text-xs leading-relaxed text-foreground">
                    {EMAIL_COMPACT}
                  </pre>
                </div>
              ) : (
                <div className="relative">
                  <div className="absolute top-2 right-2">
                    <CopyButton text={EMAIL_NATIVE} label="Copy Native Schema" />
                  </div>
                  <pre className="max-h-64 overflow-auto rounded-md border border-border bg-muted/50 p-3 font-mono text-2xs leading-relaxed text-foreground">
                    {EMAIL_NATIVE}
                  </pre>
                </div>
              )}
            </CardContent>
          </Card>
        </div>
      </section>

      {/* 4. Public Evaluation Cases */}
      <Panel
        title="Public Evaluation Cases"
        subtitle="Static verified evaluation values measured on the public benchmark suite."
        actions={
          <Badge variant="secondary">
            Tool definitions: 573 → 157 tokens (<span className="text-success font-semibold">72.60%</span>)
          </Badge>
        }
      >
        <div className="rounded-md border border-border">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="w-24">Case</TableHead>
                <TableHead>Description</TableHead>
                <TableHead className="text-right">Native</TableHead>
                <TableHead className="text-right">Compact</TableHead>
                <TableHead className="text-right">Saved</TableHead>
                <TableHead className="text-right">Reduction</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {EVAL_CASES.map((row) => (
                <TableRow key={row.id}>
                  <TableCell className="font-mono text-xs font-semibold">{row.id}</TableCell>
                  <TableCell className="text-sm">{row.description}</TableCell>
                  <TableCell className="font-mono text-xs text-right tabular-nums text-muted-foreground">
                    {row.nativeTokens} tokens
                  </TableCell>
                  <TableCell className="font-mono text-xs text-right tabular-nums text-foreground font-medium">
                    {row.compactTokens} tokens
                  </TableCell>
                  <TableCell className="font-mono text-xs text-right tabular-nums text-success font-medium">
                    -{row.saved} tokens
                  </TableCell>
                  <TableCell className="text-right font-medium text-xs">
                    <Badge variant="success">{row.reduction}</Badge>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </Panel>

      {/* 5. Tool Call Decoder */}
      <Panel
        title="Tool Call Decoder"
        subtitle="Validates deterministic decoding from raw model syntax back to standard ToolCall format."
        actions={<Badge variant="success">Decoder format validated</Badge>}
      >
        <div className="space-y-4">
          <p className="text-xs text-muted-foreground">
            Deterministic evaluation demonstration, not a live provider request. The parser processes
            streaming tokens without regex backtracking and validates JSON integrity.
          </p>

          <div className="grid grid-cols-1 items-stretch gap-3 lg:grid-cols-2">
            {/* Model output */}
            <div className="flex flex-col gap-2 rounded-md border border-border bg-card p-3">
              <div className="flex items-center justify-between">
                <span className="text-xs font-medium text-muted-foreground">Model Output</span>
                <Badge variant="outline" className="font-mono text-2xs">
                  &lt;&lt;call ...&gt;&gt;
                </Badge>
              </div>
              <div className="relative flex-1">
                <pre className="h-full overflow-x-auto rounded border border-border bg-muted/50 p-2.5 font-mono text-xs text-foreground">
                  {RAW_MODEL_OUTPUT}
                </pre>
              </div>
            </div>

            {/* Decoded ToolCall */}
            <div className="flex flex-col gap-2 rounded-md border border-border bg-card p-3">
              <div className="flex items-center justify-between">
                <span className="text-xs font-medium text-muted-foreground">
                  Decoded ToolCall
                </span>
                <Badge variant="success" className="text-2xs">
                  Standard ToolCall
                </Badge>
              </div>
              <div className="space-y-2 rounded border border-border bg-muted/50 p-2.5">
                <div className="flex items-baseline gap-2">
                  <span className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                    name:
                  </span>
                  <code className="rounded bg-background px-1.5 py-0.5 font-mono text-xs font-medium text-primary">
                    create_calendar_event
                  </code>
                </div>
                <div>
                  <span className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                    arguments:
                  </span>
                  <pre className="mt-1 overflow-x-auto rounded border border-border bg-background p-2 font-mono text-2xs text-foreground">
                    {DECODED_ARGUMENTS}
                  </pre>
                </div>
              </div>
            </div>
          </div>
        </div>
      </Panel>

      {/* 6. Fail-Closed Validation */}
      <Panel
        title="Fail-Closed Validation"
        subtitle="Guarantees strict safety invariants and graceful fallbacks during compaction and decoding."
        actions={<Badge variant="secondary">Deterministic Evaluation</Badge>}
      >
        <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2 lg:grid-cols-3">
          {VALIDATION_CHECKS.map((item, idx) => (
            <div
              key={idx}
              className="flex items-start gap-2.5 rounded-md border border-border bg-muted/30 p-2.5"
            >
              <div className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full bg-success/15 text-success">
                <Check className="size-3" />
              </div>
              <div className="min-w-0 space-y-0.5">
                <p className="font-mono text-xs font-medium text-foreground">{item.title}</p>
                <p className="text-2xs text-muted-foreground leading-normal">{item.desc}</p>
              </div>
            </div>
          ))}
        </div>
      </Panel>

      {/* 7. Runtime Status */}
      <Panel
        title="Runtime Status"
        subtitle="Current verification status for Track P1 across evaluation environments."
      >
        <div className="space-y-4">
          <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3 lg:grid-cols-5">
            <div className="rounded-md border border-border bg-muted/40 p-3 space-y-1.5">
              <span className="text-2xs text-muted-foreground">P1 Implementation</span>
              <div>
                <Badge variant="success">✓ Complete</Badge>
              </div>
            </div>

            <div className="rounded-md border border-border bg-muted/40 p-3 space-y-1.5">
              <span className="text-2xs text-muted-foreground">Deterministic Evaluation</span>
              <div>
                <Badge variant="success">✓ 8/8 cases</Badge>
              </div>
            </div>

            <div className="rounded-md border border-border bg-muted/40 p-3 space-y-1.5">
              <span className="text-2xs text-muted-foreground">Token Benchmark</span>
              <div>
                <Badge variant="success">✓ Verified</Badge>
              </div>
            </div>

            <div className="rounded-md border border-border bg-muted/40 p-3 space-y-1.5">
              <span className="text-2xs text-muted-foreground">Live Rust Runtime</span>
              <div>
                <Badge variant="warning">⚠ Local blocked</Badge>
              </div>
            </div>

            <div className="rounded-md border border-border bg-muted/40 p-3 space-y-1.5">
              <span className="text-2xs text-muted-foreground">Live Provider Test</span>
              <div>
                <Badge variant="muted">Not tested</Badge>
              </div>
            </div>
          </div>

          <Alert className="border-warning/40 bg-warning/5 text-foreground [&>svg]:text-warning">
            <AlertTriangle className="size-4" />
            <AlertTitle className="text-xs font-semibold">Local Environment Status</AlertTitle>
            <AlertDescription className="text-xs text-muted-foreground">
              The P1 implementation and deterministic public evaluation are complete. Live
              Rust/runtime verification is currently blocked on this development machine because
              the Windows MSVC linker/SDK is unavailable.
            </AlertDescription>
          </Alert>
        </div>
      </Panel>

      {/* 8. Architecture */}
      <Panel
        title="Architecture"
        subtitle="Call-path placement within the Nasiko control-plane LLM router."
        actions={
          <div className="flex flex-wrap items-center gap-1.5">
            <Badge variant="outline" className="font-mono text-2xs">
              NASIKO_COMPACT_TOOLS_ENABLED
            </Badge>
          </div>
        }
      >
        <div className="space-y-4">
          <div className="rounded-md border border-border bg-muted/30 p-4">
            <pre className="overflow-x-auto font-mono text-xs leading-relaxed text-foreground">
{`Client
  │
  ▼
Nasiko LLM Router
  │
  ▼
P1 Compact Tool Schemas
  │
  ├── Encode tool definitions
  ├── Inject compact representation
  └── Decode <<call ...>>
  │
  ▼
Upstream LLM Provider`}
            </pre>
          </div>

          <div className="flex flex-wrap items-center justify-between gap-2 border-t border-border pt-3 text-xs text-muted-foreground">
            <div className="flex items-center gap-1.5">
              <span className="size-1.5 rounded-full bg-primary" />
              <span>Opt-in via <code className="font-mono font-medium text-foreground">NASIKO_COMPACT_TOOLS_ENABLED</code></span>
            </div>
            <span>Default behavior remains unchanged when disabled.</span>
          </div>
        </div>
      </Panel>
    </div>
  )
}
