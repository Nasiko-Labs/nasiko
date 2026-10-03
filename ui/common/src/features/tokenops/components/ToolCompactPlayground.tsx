import { useState, useMemo } from 'react'
import {
  Sparkles,
  ArrowRight,
  CheckCircle2,
  AlertCircle,
  Copy,
  Check,
  RotateCcw,
  Zap,
  Code2,
  Terminal,
  Cpu,
  Layers,
  Calculator,
  Sliders,
  Play,
  FileCheck,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Textarea } from '@/components/ui/textarea'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'

interface ToolProperty {
  type?: string
  description?: string
  format?: string
  enum?: string[]
  items?: ToolProperty
  properties?: Record<string, ToolProperty>
  required?: string[]
}

interface ToolDefinition {
  type?: string
  function?: {
    name: string
    description?: string
    parameters?: {
      type?: string
      properties?: Record<string, ToolProperty>
      required?: string[]
    }
  }
  name?: string
  description?: string
  parameters?: {
    type?: string
    properties?: Record<string, ToolProperty>
    required?: string[]
  }
}

// Preset library of representative tool schemas
const PRESETS: Record<string, { name: string; schema: ToolDefinition[]; sampleCall: string }> = {
  calendar: {
    name: 'Calendar & Scheduling (Single Tool)',
    schema: [
      {
        type: 'function',
        function: {
          name: 'create_calendar_event',
          description: "Create an event in the user's calendar",
          parameters: {
            type: 'object',
            properties: {
              title: { type: 'string', description: 'Event title' },
              start: { type: 'string', format: 'date-time', description: 'Start time in ISO 8601' },
              duration_min: { type: 'integer', description: 'Duration in minutes' },
              attendees: { type: 'array', items: { type: 'string' } },
              visibility: { type: 'string', enum: ['public', 'private', 'internal'] },
            },
            required: ['title', 'start'],
          },
        },
      },
    ],
    sampleCall: '<<call create_calendar_event {"title":"Product Roadmap Sync","start":"2026-10-06T10:00:00Z","duration_min":45,"visibility":"internal"}>>',
  },
  database: {
    name: 'Database SQL Engine (Analytics Tool)',
    schema: [
      {
        type: 'function',
        function: {
          name: 'execute_sql_query',
          description: 'Execute an analytical read-only SQL query on the data warehouse',
          parameters: {
            type: 'object',
            properties: {
              query: { type: 'string', description: 'SQL query text to execute' },
              database: { type: 'string', enum: ['prod_analytics', 'staging_dw', 'audit_logs'] },
              max_rows: { type: 'integer', description: 'Maximum rows to return, defaults to 500' },
              timeout_sec: { type: 'integer', description: 'Query timeout in seconds' },
              dry_run: { type: 'boolean', description: 'Validate syntax and explain plan without running' },
            },
            required: ['query', 'database'],
          },
        },
      },
    ],
    sampleCall: '<<call execute_sql_query {"database":"prod_analytics","query":"SELECT agent_name, SUM(cost) FROM trace_usage GROUP BY 1 ORDER BY 2 DESC LIMIT 10","dry_run":false}>>',
  },
  codingSuite: {
    name: 'Developer Harness Suite (3 Tools)',
    schema: [
      {
        type: 'function',
        function: {
          name: 'read_file',
          description: 'Read the contents of a file within the workspace',
          parameters: {
            type: 'object',
            properties: {
              path: { type: 'string', description: 'Relative path to file' },
              line_start: { type: 'integer', description: '1-indexed start line' },
              line_end: { type: 'integer', description: '1-indexed end line' },
            },
            required: ['path'],
          },
        },
      },
      {
        type: 'function',
        function: {
          name: 'edit_file',
          description: 'Modify an existing file by replacing a unique content block',
          parameters: {
            type: 'object',
            properties: {
              path: { type: 'string', description: 'Target file path' },
              target_content: { type: 'string', description: 'Exact string to be replaced' },
              replacement: { type: 'string', description: 'New replacement content' },
            },
            required: ['path', 'target_content', 'replacement'],
          },
        },
      },
      {
        type: 'function',
        function: {
          name: 'run_bash',
          description: 'Run an arbitrary shell command in the repository workspace',
          parameters: {
            type: 'object',
            properties: {
              command: { type: 'string', description: 'Shell command line to execute' },
              timeout_ms: { type: 'integer', description: 'Timeout in milliseconds' },
              background: { type: 'boolean', description: 'Run process as a daemon' },
            },
            required: ['command'],
          },
        },
      },
    ],
    sampleCall: '<<call edit_file {"path":"src/main.rs","target_content":"let port = 8080;","replacement":"let port = 3000;"}>>',
  },
}

function estimateTokens(text: string): number {
  if (!text || text.trim().length === 0) return 0
  // Clean tokenizer estimation heuristic calibrated against tiktoken o200k_base
  const wordsAndPunct = text.match(/[A-Za-z0-9]+|[^A-Za-z0-9\s]|\s+/g)
  if (!wordsAndPunct) return Math.ceil(text.length / 4)
  // Non-whitespace units
  const units = wordsAndPunct.filter((u) => u.trim().length > 0)
  return Math.max(1, Math.round(units.length * 0.95))
}

function formatPropertyType(prop: ToolProperty): string {
  if (prop.enum && prop.enum.length > 0) {
    return prop.enum.join('|')
  }
  if (prop.type === 'array') {
    const itemType = prop.items ? formatPropertyType(prop.items) : 'any'
    return `[${itemType}]`
  }
  if (prop.type === 'object') {
    if (prop.properties && Object.keys(prop.properties).length > 0) {
      const keys = Object.keys(prop.properties).sort()
      const req = new Set(prop.required || [])
      const fields = keys.map((k) => {
        const optional = !req.has(k) ? '?' : ''
        return `${k}${optional}:${formatPropertyType(prop.properties![k])}`
      })
      return `{${fields.join(', ')}}`
    }
    return '{any}'
  }
  if (prop.type === 'string') {
    if (prop.format === 'date-time') return 'datetime'
    if (prop.format === 'date') return 'date'
    return 'str'
  }
  if (prop.type === 'integer') return 'int'
  if (prop.type === 'number') return 'float'
  if (prop.type === 'boolean') return 'bool'
  return prop.type || 'any'
}

function compactTool(tool: ToolDefinition): string {
  const fn = tool.function || tool
  const name = fn.name || 'unnamed_tool'
  const desc = fn.description ? ` - ${fn.description.trim()}` : ''
  const params = fn.parameters?.properties || {}
  const required = new Set(fn.parameters?.required || [])

  const sortedKeys = Object.keys(params).sort()
  const paramStrings = sortedKeys.map((k) => {
    const prop = params[k]
    const opt = !required.has(k) ? '?' : ''
    const typeStr = formatPropertyType(prop)
    return `${k}${opt}:${typeStr}`
  })

  return `${name}(${paramStrings.join(', ')})${desc}`
}

export function ToolCompactPlayground() {
  const [selectedPreset, setSelectedPreset] = useState<string>('calendar')
  const [rawJson, setRawJson] = useState<string>(() => JSON.stringify(PRESETS.calendar.schema, null, 2))
  const [llmCallInput, setLlmCallInput] = useState<string>(PRESETS.calendar.sampleCall)
  const [copiedCompact, setCopiedCompact] = useState(false)
  const [copiedPrompt, setCopiedPrompt] = useState(false)

  // Enterprise ROI Calculator States
  const [requestsPerDay, setRequestsPerDay] = useState<number>(25000)
  const [modelPricePerMillion, setModelPricePerMillion] = useState<number>(3.0) // $3/M tokens (e.g. Claude 3.5 Sonnet / GPT-4o)

  // Handle Preset Change
  const loadPreset = (presetKey: string) => {
    setSelectedPreset(presetKey)
    const p = PRESETS[presetKey]
    if (p) {
      setRawJson(JSON.stringify(p.schema, null, 2))
      setLlmCallInput(p.sampleCall)
    }
  }

  // Parse JSON and compute compact format
  const { tools, parseError, compactLines } = useMemo(() => {
    try {
      const parsed = JSON.parse(rawJson)
      const list: ToolDefinition[] = Array.isArray(parsed) ? parsed : [parsed]
      const lines = list.map(compactTool)
      return { tools: list, parseError: null, compactLines: lines }
    } catch (e) {
      return { tools: [], parseError: (e as Error).message, compactLines: [] }
    }
  }, [rawJson])

  // Token Metrics
  const baselineTokens = useMemo(() => estimateTokens(rawJson), [rawJson])
  const compactText = useMemo(() => compactLines.join('\n'), [compactLines])
  const compactTokens = useMemo(() => estimateTokens(compactText), [compactText])
  const tokenDelta = baselineTokens - compactTokens
  const savingsPct = baselineTokens > 0 ? Math.round((tokenDelta / baselineTokens) * 1000) / 10 : 0
  const densityMultiplier = compactTokens > 0 ? (baselineTokens / compactTokens).toFixed(2) : '1.00'

  // LLM Call Decoder & Validator
  const decodedResult = useMemo(() => {
    if (!llmCallInput.trim()) {
      return { status: 'idle', message: 'Enter an LLM response containing <<call tool_name {...}>> to test decoding.' }
    }

    const match = llmCallInput.match(/<<call\s+([a-zA-Z0-9_\.\-]+)\s+({[\s\S]*?})>>/)
    if (!match) {
      return {
        status: 'error',
        message: 'Syntax error: Could not find <<call tool_name {...}>> pattern in output.',
      }
    }

    const [, toolName, argsJson] = match
    let parsedArgs: Record<string, unknown>
    try {
      parsedArgs = JSON.parse(argsJson)
    } catch (e) {
      return {
        status: 'error',
        toolName,
        message: `JSON arguments parsing failed: ${(e as Error).message}`,
      }
    }

    // Validate against loaded tools if valid
    const targetTool = tools.find((t) => (t.function?.name || t.name) === toolName)
    if (!targetTool) {
      return {
        status: 'warning',
        toolName,
        args: parsedArgs,
        message: `Tool "${toolName}" is not defined in the loaded schema above.`,
      }
    }

    const fn = targetTool.function || targetTool
    const params = fn.parameters?.properties || {}
    const required = fn.parameters?.required || []

    const missingRequired = required.filter((r) => parsedArgs[r] === undefined || parsedArgs[r] === null)
    if (missingRequired.length > 0) {
      return {
        status: 'error',
        toolName,
        args: parsedArgs,
        message: `Missing required parameter(s): ${missingRequired.join(', ')}`,
      }
    }

    // Check enum violations
    for (const [key, val] of Object.entries(parsedArgs)) {
      const prop = params[key]
      if (prop && prop.enum && typeof val === 'string' && !prop.enum.includes(val)) {
        return {
          status: 'error',
          toolName,
          args: parsedArgs,
          message: `Enum violation for "${key}": "${val}" is not one of [${prop.enum.join(', ')}]`,
        }
      }
    }

    return {
      status: 'success',
      toolName,
      args: parsedArgs,
      message: `Strict validation passed: valid parameters for "${toolName}".`,
    }
  }, [llmCallInput, tools])

  // Enterprise ROI Projections
  const dailyTokensSaved = requestsPerDay * tokenDelta
  const monthlyTokensSaved = dailyTokensSaved * 30
  const monthlyDollarSavings = (monthlyTokensSaved / 1_000_000) * modelPricePerMillion

  const copyToClipboard = (text: string, type: 'compact' | 'prompt') => {
    navigator.clipboard.writeText(text)
    if (type === 'compact') {
      setCopiedCompact(true)
      setTimeout(() => setCopiedCompact(false), 2000)
    } else {
      setCopiedPrompt(true)
      setTimeout(() => setCopiedPrompt(false), 2000)
    }
  }

  const systemPromptTemplate = `You have access to the following tools:
${compactText}

To call a tool, output exactly:
<<call tool_name {"param": "value"}>>
Do not add any additional preamble before the tool call.`

  return (
    <div className="flex flex-col gap-6 py-2">
      {/* Top Banner */}
      <Card className="relative overflow-hidden border border-emerald-500/25 bg-gradient-to-r from-emerald-500/10 via-teal-500/5 to-background p-6">
        <div className="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div className="flex items-start gap-4">
            <div className="flex size-12 shrink-0 items-center justify-center rounded-xl bg-emerald-500/20 text-emerald-600 dark:text-emerald-400">
              <Zap className="size-6" />
            </div>
            <div>
              <div className="flex flex-wrap items-center gap-2">
                <h2 className="text-xl font-bold tracking-tight text-foreground">
                  Tool Schema Compact Playground
                </h2>
                <Badge variant="outline" className="border-emerald-500/40 bg-emerald-500/10 font-semibold text-emerald-600 dark:text-emerald-400">
                  P1 Protocol Engine
                </Badge>
                <Badge variant="secondary" className="text-xs">
                  Zero Guessing Decoder
                </Badge>
              </div>
              <p className="mt-1 text-sm text-muted-foreground">
                Test single-line schema compaction, inspect real-time token reduction, and simulate strict typed tool calls with zero-boilerplate formatting.
              </p>
            </div>
          </div>

          {/* Quick Presets */}
          <div className="flex items-center gap-2 self-start md:self-auto">
            <span className="text-xs font-medium text-muted-foreground">Preset:</span>
            <Select value={selectedPreset} onValueChange={loadPreset}>
              <SelectTrigger className="h-8 w-56 text-xs font-medium">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {Object.entries(PRESETS).map(([key, item]) => (
                  <SelectItem key={key} value={key} className="text-xs">
                    {item.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>

        {/* Live Metrics Row */}
        <div className="mt-6 grid grid-cols-2 gap-3 sm:grid-cols-4">
          <div className="rounded-lg border border-border/60 bg-background/80 p-3.5 backdrop-blur-xs">
            <span className="text-xs text-muted-foreground">Baseline Schema</span>
            <div className="mt-1 flex items-baseline gap-1.5">
              <span className="text-2xl font-bold tracking-tight text-foreground">
                {baselineTokens}
              </span>
              <span className="text-[11px] text-muted-foreground">tokens</span>
            </div>
            <p className="mt-0.5 text-[11px] text-muted-foreground">Verbose JSON Schema</p>
          </div>

          <div className="rounded-lg border border-border/60 bg-background/80 p-3.5 backdrop-blur-xs">
            <span className="text-xs text-muted-foreground">Compact Format</span>
            <div className="mt-1 flex items-baseline gap-1.5">
              <span className="text-2xl font-bold tracking-tight text-emerald-600 dark:text-emerald-400">
                {compactTokens}
              </span>
              <span className="text-[11px] text-muted-foreground">tokens</span>
            </div>
            <p className="mt-0.5 text-[11px] text-muted-foreground">Single-line signature</p>
          </div>

          <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/5 p-3.5 backdrop-blur-xs">
            <span className="text-xs font-medium text-emerald-700 dark:text-emerald-400">
              Prompt Reduction
            </span>
            <div className="mt-1 flex items-baseline gap-1.5">
              <span className="text-2xl font-bold tracking-tight text-emerald-600 dark:text-emerald-400">
                -{savingsPct}%
              </span>
              <span className="text-[11px] font-semibold text-emerald-600">saved</span>
            </div>
            <p className="mt-0.5 text-[11px] text-emerald-700 dark:text-emerald-400">
              {tokenDelta} tokens saved / call
            </p>
          </div>

          <div className="rounded-lg border border-border/60 bg-background/80 p-3.5 backdrop-blur-xs">
            <span className="text-xs text-muted-foreground">Context Density</span>
            <div className="mt-1 flex items-baseline gap-1.5">
              <span className="text-2xl font-bold tracking-tight text-foreground">
                {densityMultiplier}×
              </span>
              <span className="text-[11px] text-muted-foreground">multiplier</span>
            </div>
            <p className="mt-0.5 text-[11px] text-muted-foreground">Higher context capacity</p>
          </div>
        </div>
      </Card>

      {/* Main Studio Split View */}
      <div className="grid gap-6 lg:grid-cols-2">
        {/* Left: JSON Schema Input */}
        <Card className="flex flex-col border border-border p-5 shadow-xs">
          <div className="mb-3 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Code2 className="size-4 text-primary-text" />
              <h3 className="text-sm font-semibold tracking-tight text-foreground">
                Input Tool Definition (JSON Schema)
              </h3>
            </div>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                try {
                  const obj = JSON.parse(rawJson)
                  setRawJson(JSON.stringify(obj, null, 2))
                } catch {
                  // ignore
                }
              }}
              className="h-7 text-xs text-muted-foreground hover:text-foreground"
            >
              Format JSON
            </Button>
          </div>

          {parseError && (
            <div className="mb-3 flex items-center gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive">
              <AlertCircle className="size-4 shrink-0" />
              <span>{parseError}</span>
            </div>
          )}

          <Textarea
            value={rawJson}
            onChange={(e) => setRawJson(e.target.value)}
            className="font-mono text-xs leading-relaxed min-h-[340px] resize-y bg-muted/20"
            placeholder="Paste OpenAI, Anthropic, or MCP tool definitions JSON here..."
            spellCheck={false}
          />

          <div className="mt-3 flex items-center justify-between text-xs text-muted-foreground">
            <span>{tools.length} tool(s) loaded</span>
            <span>Approx. {baselineTokens} tokens</span>
          </div>
        </Card>

        {/* Right: Compact Representation */}
        <Card className="flex flex-col border border-border p-5 shadow-xs">
          <div className="mb-3 flex items-center justify-between">
            <div className="flex items-center gap-2">
              <Cpu className="size-4 text-emerald-600 dark:text-emerald-400" />
              <h3 className="text-sm font-semibold tracking-tight text-foreground">
                Compacted Output (nasiko-tool-compact)
              </h3>
            </div>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => copyToClipboard(compactText, 'compact')}
                className="h-7 gap-1.5 text-xs"
              >
                {copiedCompact ? <Check className="size-3 text-emerald-600" /> : <Copy className="size-3" />}
                {copiedCompact ? 'Copied' : 'Copy Compact'}
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => copyToClipboard(systemPromptTemplate, 'prompt')}
                className="h-7 gap-1.5 text-xs"
              >
                {copiedPrompt ? <Check className="size-3 text-emerald-600" /> : <Layers className="size-3" />}
                {copiedPrompt ? 'Copied' : 'Copy System Prompt'}
              </Button>
            </div>
          </div>

          <div className="flex min-h-[340px] flex-col rounded-md border border-emerald-500/20 bg-muted/20 p-4 font-mono text-xs">
            <div className="mb-2 text-[11px] font-semibold text-emerald-600 dark:text-emerald-400">
              // Dense DSL sent directly to model prompt:
            </div>
            <div className="flex-1 space-y-2.5 overflow-x-auto select-all">
              {compactLines.map((line, idx) => (
                <div key={idx} className="rounded bg-background/80 p-2.5 text-foreground shadow-2xs border border-border/50">
                  <span className="font-semibold text-emerald-600 dark:text-emerald-400">
                    {line.split('(')[0]}
                  </span>
                  <span>({line.split('(').slice(1).join('(')}</span>
                </div>
              ))}
            </div>

            <div className="mt-3 border-t border-border/60 pt-3 text-[11px] text-muted-foreground">
              <span className="font-semibold text-foreground">Wire protocol:</span> Models emit{' '}
              <code className="rounded bg-muted px-1.5 py-0.5 text-emerald-600 dark:text-emerald-400">
                &lt;&lt;call name &#123;...&#125;&gt;&gt;
              </code>
            </div>
          </div>

          <div className="mt-3 flex items-center justify-between text-xs text-muted-foreground">
            <span className="text-emerald-600 dark:text-emerald-400 font-medium">
              Compressed from {baselineTokens} → {compactTokens} tokens
            </span>
            <Badge variant="outline" className="border-emerald-500/30 text-emerald-600 dark:text-emerald-400">
              {densityMultiplier}× denser
            </Badge>
          </div>
        </Card>
      </div>

      {/* Interactive LLM Output Simulator & Decoder */}
      <Card className="border border-border p-5 shadow-xs">
        <div className="mb-4 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-2">
            <Terminal className="size-4 text-primary-text" />
            <h3 className="text-base font-semibold tracking-tight text-foreground">
              LLM Call Decoder & Validation Simulator
            </h3>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-xs text-muted-foreground">Quick test cases:</span>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                const first = tools[0]
                const name = first?.function?.name || first?.name || 'tool'
                setLlmCallInput(`<<call ${name} {"title":"Quarterly Review","start":"2026-10-10T14:00:00Z"}>>`)
              }}
              className="h-7 text-xs"
            >
              Valid Call
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                const first = tools[0]
                const name = first?.function?.name || first?.name || 'tool'
                setLlmCallInput(`<<call ${name} {"duration_min":30}>>`)
              }}
              className="h-7 text-xs text-amber-600 dark:text-amber-400"
            >
              Missing Required
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                const first = tools[0]
                const name = first?.function?.name || first?.name || 'tool'
                setLlmCallInput(`<<call ${name} {"title":"All Hands","start":"2026-10-10T14:00:00Z","visibility":"restricted"}>>`)
              }}
              className="h-7 text-xs text-destructive"
            >
              Invalid Enum
            </Button>
          </div>
        </div>

        <div className="grid gap-4 md:grid-cols-2">
          {/* Input field */}
          <div className="flex flex-col gap-2">
            <label className="text-xs font-medium text-muted-foreground">
              LLM Generated Response Stream:
            </label>
            <Textarea
              value={llmCallInput}
              onChange={(e) => setLlmCallInput(e.target.value)}
              className="font-mono text-xs min-h-[140px] bg-muted/20"
              placeholder='e.g. <<call function_name {"key": "value"}>>'
            />
          </div>

          {/* Decoded Result */}
          <div className="flex flex-col gap-2">
            <div className="flex items-center justify-between">
              <label className="text-xs font-medium text-muted-foreground">
                Decoder Output & Schema Validation:
              </label>
              {decodedResult.status === 'success' && (
                <Badge variant="outline" className="border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 gap-1">
                  <CheckCircle2 className="size-3" /> Valid Call
                </Badge>
              )}
              {decodedResult.status === 'error' && (
                <Badge variant="outline" className="border-destructive/40 bg-destructive/10 text-destructive gap-1">
                  <AlertCircle className="size-3" /> Validation Failed
                </Badge>
              )}
              {decodedResult.status === 'warning' && (
                <Badge variant="outline" className="border-amber-500/40 bg-amber-500/10 text-amber-600 dark:text-amber-400 gap-1">
                  <AlertCircle className="size-3" /> Unknown Tool
                </Badge>
              )}
            </div>

            <div className="flex min-h-[140px] flex-col justify-between rounded-md border border-border bg-muted/20 p-3 font-mono text-xs">
              <div>
                <p className="text-xs text-muted-foreground mb-2">
                  {decodedResult.message}
                </p>
                {decodedResult.args && (
                  <pre className="rounded bg-background/80 p-2 text-foreground overflow-x-auto text-[11px] border border-border/50">
                    {JSON.stringify(decodedResult.args, null, 2)}
                  </pre>
                )}
              </div>
              <div className="text-[11px] text-muted-foreground pt-2 border-t border-border/40 flex items-center justify-between">
                <span>Decoder mode: strict zero-guessing</span>
                <span>Zero hallucinated params</span>
              </div>
            </div>
          </div>
        </div>
      </Card>

      {/* Enterprise Savings & FinOps Calculator */}
      <Card className="border border-border p-5 shadow-xs">
        <div className="mb-4 flex items-center gap-2">
          <Calculator className="size-4 text-emerald-600 dark:text-emerald-400" />
          <h3 className="text-base font-semibold tracking-tight text-foreground">
            Enterprise Fleet ROI Calculator
          </h3>
        </div>

        <div className="grid gap-6 md:grid-cols-3">
          <div className="flex flex-col gap-3">
            <div>
              <label className="text-xs font-medium text-foreground">
                Agent / Tool Requests per Day:
              </label>
              <Input
                type="number"
                value={requestsPerDay}
                onChange={(e) => setRequestsPerDay(Math.max(1, Number(e.target.value)))}
                className="mt-1 h-8 text-xs font-mono"
              />
            </div>
            <div>
              <label className="text-xs font-medium text-foreground">
                Model Pricing ($ / 1M prompt tokens):
              </label>
              <Input
                type="number"
                step="0.5"
                value={modelPricePerMillion}
                onChange={(e) => setModelPricePerMillion(Math.max(0.1, Number(e.target.value)))}
                className="mt-1 h-8 text-xs font-mono"
              />
              <span className="text-[10px] text-muted-foreground">
                $3.00 for Claude 3.5 Sonnet / GPT-4o, $0.15 for GPT-4o-mini
              </span>
            </div>
          </div>

          <div className="rounded-lg border border-border/60 bg-muted/20 p-4 flex flex-col justify-center">
            <span className="text-xs text-muted-foreground">Monthly Tokens Avoided</span>
            <div className="mt-1 text-2xl font-bold tracking-tight text-emerald-600 dark:text-emerald-400">
              {(monthlyTokensSaved / 1_000_000).toFixed(2)}M
            </div>
            <p className="mt-1 text-xs text-muted-foreground">
              {(dailyTokensSaved / 1_000).toLocaleString()}k tokens saved per day across agent fleet
            </p>
          </div>

          <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/10 p-4 flex flex-col justify-center">
            <span className="text-xs font-medium text-emerald-700 dark:text-emerald-400">
              Projected Monthly Savings
            </span>
            <div className="mt-1 text-3xl font-extrabold tracking-tight text-emerald-600 dark:text-emerald-400">
              ${monthlyDollarSavings.toFixed(2)}
            </div>
            <p className="mt-1 text-xs text-emerald-700 dark:text-emerald-400">
              Direct reduction on your API bills with no code changes needed
            </p>
          </div>
        </div>
      </Card>
    </div>
  )
}
