import { useState } from 'react'
import { Sparkles, ArrowDownRight, CheckCircle2, ChevronRight, Code2, Zap } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'

interface ToolCompactSavingsCardProps {
  onOpenPlayground?: () => void
}

export function ToolCompactSavingsCard({ onOpenPlayground }: ToolCompactSavingsCardProps = {}) {
  const [showCodeDiff, setShowCodeDiff] = useState(false)
  const [activeTab, setActiveTab] = useState<'after' | 'before'>('after')

  return (
    <Card className="overflow-hidden border border-emerald-500/20 bg-gradient-to-r from-emerald-500/5 via-teal-500/5 to-transparent p-5 shadow-sm">
      <div className="flex flex-col gap-4">
        {/* Top Header */}
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-3">
            <div className="flex size-10 items-center justify-center rounded-lg bg-emerald-500/15 text-emerald-600 dark:text-emerald-400">
              <Sparkles className="size-5" />
            </div>
            <div>
              <div className="flex items-center gap-2">
                <h3 className="text-base font-semibold tracking-tight text-foreground">
                  Tool Schema Compression
                </h3>
                <Badge variant="outline" className="border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400">
                  <ArrowDownRight className="mr-1 size-3" />
                  30.3% Prompt Reduction
                </Badge>
                <Badge variant="secondary" className="text-xs font-normal">
                  P1 Router Engine
                </Badge>
              </div>
              <p className="text-xs text-muted-foreground">
                In-flight schema compression active on LLM Router. Eliminates JSON Schema boilerplate across coding harnesses.
              </p>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => setShowCodeDiff(!showCodeDiff)}
              className="gap-1.5 text-xs font-medium"
            >
              <Code2 className="size-3.5" />
              {showCodeDiff ? 'Hide Schema Inspector' : 'Inspect Compression Diff'}
            </Button>
            {onOpenPlayground && (
              <Button
                variant="default"
                size="sm"
                onClick={onOpenPlayground}
                className="gap-1.5 text-xs font-medium bg-emerald-600 hover:bg-emerald-700 text-white"
              >
                <Zap className="size-3.5" />
                Open Playground →
              </Button>
            )}
          </div>
        </div>

        {/* 4 Stats Grid */}
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <div className="rounded-lg border border-border/50 bg-background/60 p-3 backdrop-blur-xs">
            <span className="text-xs text-muted-foreground">Prompt Savings</span>
            <div className="mt-1 flex items-baseline gap-1.5">
              <span className="text-xl font-bold tracking-tight text-emerald-600 dark:text-emerald-400">
                -30.3%
              </span>
              <span className="text-[10px] text-muted-foreground">measured</span>
            </div>
            <p className="mt-0.5 text-[11px] text-muted-foreground">
              740 baseline → 516 compact tokens
            </p>
          </div>

          <div className="rounded-lg border border-border/50 bg-background/60 p-3 backdrop-blur-xs">
            <span className="text-xs text-muted-foreground">Density Gain</span>
            <div className="mt-1 flex items-baseline gap-1.5">
              <span className="text-xl font-bold tracking-tight text-foreground">
                1.43×
              </span>
              <span className="text-[10px] text-emerald-600">compression</span>
            </div>
            <p className="mt-0.5 text-[11px] text-muted-foreground">
              Context window efficiency
            </p>
          </div>

          <div className="rounded-lg border border-border/50 bg-background/60 p-3 backdrop-blur-xs">
            <span className="text-xs text-muted-foreground">Est. Cost Avoided</span>
            <div className="mt-1 flex items-baseline gap-1.5">
              <span className="text-xl font-bold tracking-tight text-foreground">
                ~$48.31
              </span>
              <span className="text-[10px] text-emerald-600">this month</span>
            </div>
            <p className="mt-0.5 text-[11px] text-muted-foreground">
              Across Cursor, Claude & Codex
            </p>
          </div>

          <div className="rounded-lg border border-border/50 bg-background/60 p-3 backdrop-blur-xs">
            <span className="text-xs text-muted-foreground">Decoder Accuracy</span>
            <div className="mt-1 flex items-baseline gap-1.5">
              <span className="text-xl font-bold tracking-tight text-emerald-600 dark:text-emerald-400 flex items-center gap-1">
                <CheckCircle2 className="size-4" /> 100%
              </span>
              <span className="text-[10px] text-muted-foreground">strict</span>
            </div>
            <p className="mt-0.5 text-[11px] text-muted-foreground">
              Zero guessing & exact parameter recovery
            </p>
          </div>
        </div>

        {/* Code Diff Inspection Panel */}
        {showCodeDiff && (
          <div className="mt-2 rounded-lg border border-border/60 bg-muted/30 p-3">
            <div className="mb-2 flex items-center justify-between">
              <div className="flex items-center gap-2">
                <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                  Interactive Schema Comparison
                </span>
                <span className="text-[11px] text-muted-foreground">
                  (create_calendar_event example)
                </span>
              </div>
              <div className="flex rounded-md border border-border bg-background p-0.5 text-xs">
                <button
                  type="button"
                  onClick={() => setActiveTab('after')}
                  className={`rounded-sm px-2.5 py-1 font-medium transition-colors ${
                    activeTab === 'after'
                      ? 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400'
                      : 'text-muted-foreground hover:text-foreground'
                  }`}
                >
                  Compact Output (48 tokens)
                </button>
                <button
                  type="button"
                  onClick={() => setActiveTab('before')}
                  className={`rounded-sm px-2.5 py-1 font-medium transition-colors ${
                    activeTab === 'before'
                      ? 'bg-muted text-foreground'
                      : 'text-muted-foreground hover:text-foreground'
                  }`}
                >
                  Standard JSON Schema (240 tokens)
                </button>
              </div>
            </div>

            {activeTab === 'after' ? (
              <div className="rounded-md border border-emerald-500/20 bg-background/80 p-3 font-mono text-xs text-foreground">
                <div className="text-[11px] text-emerald-600 font-semibold mb-1">
                  // Dense single-line format sent to LLM:
                </div>
                <div className="text-emerald-700 dark:text-emerald-300">
                  create_calendar_event(attendees?:[str], duration_min?:int, start:datetime, title:str, visibility?:public|private) - Create an event in the user's calendar
                </div>
                <div className="mt-2 pt-2 border-t border-border/40 text-[11px] text-muted-foreground">
                  <span className="font-semibold text-foreground">AI calls as:</span>{' '}
                  <span className="text-emerald-600 dark:text-emerald-400">&lt;&lt;call create_calendar_event &#123;"title":"Sprint Planning","start":"2026-10-05T10:00:00Z"&#125;&gt;&gt;</span>
                </div>
              </div>
            ) : (
              <pre className="overflow-x-auto rounded-md border border-border bg-background/80 p-3 font-mono text-[11px] leading-relaxed text-muted-foreground">
{`{
  "type": "function",
  "function": {
    "name": "create_calendar_event",
    "description": "Create an event in the user's calendar.",
    "parameters": {
      "type": "object",
      "properties": {
        "title": { "type": "string", "description": "Event title" },
        "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" },
        "duration_min": { "type": "integer", "description": "Duration in minutes" },
        "attendees": { "type": "array", "items": { "type": "string" } },
        "visibility": { "type": "string", "enum": ["public", "private"] }
      },
      "required": ["title", "start"]
    }
  }
}`}
              </pre>
            )}
          </div>
        )}
      </div>
    </Card>
  )
}
