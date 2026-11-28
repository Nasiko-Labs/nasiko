/**
 * `/evals/code-review` (`oss/docs/proposals/evals/CODE_REVIEW_EVALS.md`): import one GitHub pull
 * request, optionally add human-validated reference findings, then review it with chosen LLM
 * configs. A task with no reference findings stays an ungraded preview — its attempts' findings
 * are shown, but never a precision/recall score, which would otherwise read as a real benchmark
 * result when nothing has validated it. This reviews one task at a time, not a dataset of many;
 * the sample-size warning the server always sends says so in the results, too.
 */
import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { useConfigs } from '@nasiko/features/router/api'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { fmtLatency, fmtMoneyFine, fmtPct } from '@nasiko/ui/lib/format'
import { Alert, AlertDescription, AlertTitle } from '@nasiko/ui/primitives/alert'
import { Badge } from '@nasiko/ui/primitives/badge'
import { asButton, Button } from '@nasiko/ui/primitives/button'
import { Card, CardContent, CardHeader, CardTitle } from '@nasiko/ui/primitives/card'
import { Checkbox } from '@nasiko/ui/primitives/checkbox'
import { Input } from '@nasiko/ui/primitives/input'
import { Label } from '@nasiko/ui/primitives/label'
import { Textarea } from '@nasiko/ui/primitives/textarea'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import {
  useAddReferenceFindings,
  useCreateReviewRun,
  useImportPullRequest,
  useReviewRun,
} from './api'
import { copy } from './copy'
import type { AttemptResult, ReferenceFinding, ReviewTask } from './types'

export function CodeReviewPage() {
  const [task, setTask] = useState<ReviewTask | null>(null)
  const [runId, setRunId] = useState<string | null>(null)

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-4">
      <TopBar />
      <PageHeader
        title={copy.title}
        description={copy.description}
        actions={
          <Link to="/evals/universe" {...asButton({ variant: 'outline', size: 'sm' })}>
            {copy.universeLink}
          </Link>
        }
      />
      <ImportForm
        onImported={(t) => {
          setTask(t)
          setRunId(null)
        }}
      />
      {task && (
        <TaskCard
          task={task}
          onTaskUpdated={setTask}
          onRunStarted={setRunId}
        />
      )}
      {runId && <RunResults runId={runId} />}
    </div>
  )
}

function ImportForm({ onImported }: { onImported: (task: ReviewTask) => void }) {
  const [owner, setOwner] = useState('')
  const [repo, setRepo] = useState('')
  const [prNumber, setPrNumber] = useState('')
  const importPr = useImportPullRequest()

  const submit = () => {
    const pr_number = Number.parseInt(prNumber, 10)
    if (!owner || !repo || !Number.isFinite(pr_number) || pr_number <= 0) return
    importPr.mutate(
      { owner, repo, pr_number },
      { onSuccess: onImported },
    )
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{copy.import.title}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-wrap items-end gap-3">
        <div className="flex flex-col gap-1">
          <Label htmlFor="cr-owner">{copy.import.owner}</Label>
          <Input id="cr-owner" value={owner} onChange={(e) => setOwner(e.target.value)} />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="cr-repo">{copy.import.repo}</Label>
          <Input id="cr-repo" value={repo} onChange={(e) => setRepo(e.target.value)} />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="cr-pr">{copy.import.prNumber}</Label>
          <Input
            id="cr-pr"
            inputMode="numeric"
            value={prNumber}
            onChange={(e) => setPrNumber(e.target.value)}
          />
        </div>
        <Button onClick={submit} disabled={importPr.isPending}>
          {importPr.isPending ? copy.import.importing : copy.import.action}
        </Button>
        {importPr.isError && (
          <Alert variant="destructive">
            <AlertTitle>{copy.import.failed}</AlertTitle>
            <AlertDescription>{(importPr.error as Error).message}</AlertDescription>
          </Alert>
        )}
      </CardContent>
    </Card>
  )
}

function TaskCard({
  task,
  onTaskUpdated,
  onRunStarted,
}: {
  task: ReviewTask
  onTaskUpdated: (task: ReviewTask) => void
  onRunStarted: (runId: string) => void
}) {
  const [findingsJson, setFindingsJson] = useState('')
  const [findingsError, setFindingsError] = useState<string | null>(null)
  const addFindings = useAddReferenceFindings(task.id)
  const configs = useConfigs()
  const [picked, setPicked] = useState<string[]>([])
  const createRun = useCreateReviewRun(task.id)

  const submitFindings = () => {
    setFindingsError(null)
    let parsed: ReferenceFinding[]
    try {
      parsed = JSON.parse(findingsJson)
    } catch {
      setFindingsError(copy.findings.invalidJson)
      return
    }
    addFindings.mutate(parsed, {
      onSuccess: onTaskUpdated,
      onError: () => setFindingsError(copy.findings.saveFailed),
    })
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {task.owner}/{task.repo} #{task.pr_number} — {task.pr_title}
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <span>{copy.task.files(Object.keys(task.files).length)}</span>
          <Badge variant={task.graded ? 'default' : 'secondary'}>
            {task.graded ? copy.task.graded : copy.task.ungraded}
          </Badge>
        </div>

        {task.graded ? (
          <ul className="list-inside list-disc text-sm">
            {task.reference_findings.map((f) => (
              <li key={`${f.file}:${f.line}:${f.category}:${f.description}`}>
                {f.file}:{f.line} ({f.severity}/{f.category}) — {f.description}
              </li>
            ))}
          </ul>
        ) : (
          <Alert>
            <AlertTitle>{copy.task.noReference}</AlertTitle>
            <AlertDescription>{copy.task.noReferenceBody}</AlertDescription>
          </Alert>
        )}

        <div className="flex flex-col gap-2">
          <Label htmlFor="cr-findings">{copy.findings.label}</Label>
          <Textarea
            id="cr-findings"
            className="min-h-24 font-mono text-xs"
            placeholder={copy.findings.placeholder}
            value={findingsJson}
            onChange={(e) => setFindingsJson(e.target.value)}
          />
          <Button
            size="sm"
            variant="outline"
            onClick={submitFindings}
            disabled={addFindings.isPending || !findingsJson.trim()}
          >
            {copy.findings.action}
          </Button>
          {findingsError && <p className="text-sm text-destructive">{findingsError}</p>}
        </div>

        <div className="flex flex-col gap-2">
          <Label>{copy.models.label}</Label>
          <div className="flex flex-col gap-1">
            {(configs.data ?? []).map((c) => (
              <label key={c.id} className="flex items-center gap-2 text-sm">
                <Checkbox
                  checked={picked.includes(c.id)}
                  onCheckedChange={(on) =>
                    setPicked((p) => (on ? [...p, c.id] : p.filter((id) => id !== c.id)))
                  }
                />
                {c.name} ({c.provider}
                {c.model ? `: ${c.model}` : ''})
              </label>
            ))}
          </div>
          <Button
            onClick={() =>
              createRun.mutate(picked, { onSuccess: (run) => onRunStarted(run.id) })
            }
            disabled={!picked.length || createRun.isPending}
          >
            {createRun.isPending ? copy.models.running : copy.models.action}
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}

function RunResults({ runId }: { runId: string }) {
  const run = useReviewRun(runId)
  if (run.isPending) return <p>{copy.results.loading}</p>
  if (run.isError) return <p className="text-destructive">{copy.results.failed}</p>
  const data = run.data

  return (
    <Card>
      <CardHeader>
        <CardTitle>{copy.results.title}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <Alert variant={data.graded ? 'default' : 'destructive'}>
          <AlertTitle>{data.graded ? copy.results.graded : copy.results.ungraded}</AlertTitle>
          <AlertDescription>{data.sample_size_warning}</AlertDescription>
        </Alert>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{copy.results.cols.model}</TableHead>
              <TableHead>{copy.results.cols.status}</TableHead>
              <TableHead>{copy.results.cols.precision}</TableHead>
              <TableHead>{copy.results.cols.recall}</TableHead>
              <TableHead>{copy.results.cols.cost}</TableHead>
              <TableHead>{copy.results.cols.latency}</TableHead>
              <TableHead>{copy.results.cols.priceSource}</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.results.map((r: AttemptResult) => (
              <TableRow key={r.model_id}>
                <TableCell>{r.model_name}</TableCell>
                <TableCell>{r.status}</TableCell>
                <TableCell>
                  {r.metrics ? fmtPct(r.metrics.precision * 100) : copy.results.preview}
                </TableCell>
                <TableCell>
                  {r.metrics ? fmtPct(r.metrics.recall * 100) : copy.results.preview}
                </TableCell>
                <TableCell>{fmtMoneyFine(r.cost_usd)}</TableCell>
                <TableCell>{fmtLatency(r.latency_ms)}</TableCell>
                <TableCell>{r.worst_price_source ?? '—'}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
        {data.results.some((r) => r.findings.length > 0) && (
          <div className="flex flex-col gap-3">
            {data.results.map((r) => (
              <div key={r.model_id}>
                <p className="text-sm font-medium">{r.model_name}</p>
                <ul className="list-inside list-disc text-sm">
                  {r.findings.map((f) => (
                    <li key={`${f.file}:${f.line}:${f.category}:${f.description}`}>
                      {f.file}:{f.line} ({f.severity}/{f.category}) — {f.description}
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        )}
        <p className="text-xs text-muted-foreground">{copy.results.taskCount}</p>
      </CardContent>
    </Card>
  )
}
