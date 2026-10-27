/**
 * Needs attention (plans/feat-optimization-page.md P5, R1B, R1C, R2C, R2F, R6x; eng E2, C7): a ui/alert strip shown
 * only when something you can fix needs you. Line 1: your agents with Token optimization off (your chats' history
 * compression). Line 2, superusers only: other owners' agents (their chats). Each names its agents (links to them) and
 * offers one confirmed bulk turn-on, written 4 at a time with progress; agents that worked drop off, failures stay
 * with the server's reason and Retry, and the end is one polite announcement through the page's live region.
 *
 * R2C: nothing shows until the lists settle, and a failed list says so (never an all-clear).
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { AlertTriangle } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { useAnnounce } from '@nasiko/ui/composites/announce'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Button } from '@nasiko/ui/primitives/button'
import { ScrollArea } from '@nasiko/ui/primitives/scroll-area'
import { Spinner } from '@nasiko/ui/primitives/spinner'
import {
  useAgentsDirectory,
  useOwnedAgents,
  useSetCompressionFor,
  useUsers,
} from '@nasiko/features/agents/api'
import { ConfirmDialog } from '@nasiko/features/agents/components/dialogs'
import { meQuery } from '@nasiko/data-access/auth'
import { ApiError } from '@nasiko/data-access/client'
import { tiersQuery } from '../api'
import { attentionLines, shownNames, type OffAgent } from '../attention'
import { copy } from '../copy'

type Group = 'mine' | 'others'

const reasonOf = (err: unknown) =>
  (err instanceof ApiError && err.serverMessage) || copy.attention.failedFallback

export function NeedsAttention() {
  const announce = useAnnounce()
  const me = useQuery(meQuery)
  const sub = me.data?.sub
  const superuser = me.data?.is_superuser === true
  const owned = useOwnedAgents(sub)
  const directory = useAgentsDirectory(superuser)
  const tiers = useQuery(tiersQuery)
  const turnOn = useSetCompressionFor()
  const [confirm, setConfirm] = useState<Group | null>(null)
  // Owner names are shown only in the fleet-wide confirm: read them once there is such a line (review: performance).
  const users = useUsers(
    superuser &&
      !!sub &&
      !!directory.data?.some(
        (r) =>
          r.owner_id !== sub && (r as { compress_enabled?: unknown }).compress_enabled === false,
      ),
  )
  const [running, setRunning] = useState<{ group: Group; done: number; total: number } | null>(null)
  // Agents a batch just turned on, dropped until a newer successful read of the lists says who is on. A failed
  // refetch keeps them dropped (and keeps the failure reasons) instead of bringing them back as off (review: red team).
  const [hidden, setHidden] = useState<{ ids: ReadonlySet<string>; until: number }>({
    ids: new Set(),
    until: 0,
  })
  const [failed, setFailed] = useState<ReadonlyMap<string, string>>(new Map())

  if (!sub || owned.isPending || (superuser && directory.isPending)) return null
  // Only a list that never loaded is "couldn't check": a failed refetch keeps the last good rows (R2C).
  if ((owned.isError && !owned.data) || (superuser && directory.isError && !directory.data))
    return (
      <Strip>
        <Header />
        <p className="m-0">
          {copy.attention.failedCheck}{' '}
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0 pointer-coarse:min-h-11"
            onClick={() => {
              void owned.refetch()
              if (superuser) void directory.refetch()
            }}
          >
            {copy.retry}
          </Button>
        </p>
      </Strip>
    )

  const fresh = Math.max(owned.dataUpdatedAt, superuser ? directory.dataUpdatedAt : 0)
  const { mine, others } = attentionLines({
    owned: owned.data,
    directory: directory.data,
    me: sub,
    superuser,
    hidden: fresh > hidden.until ? new Set() : hidden.ids,
  })
  if (!mine.length && !others.length) return null

  const run = (group: Group, agents: OffAgent[]) => {
    // Stamped before the writes: the batch refetch that lands after them is newer, so it decides who is on (review:
    // adversarial; the hook's refetch runs before this call's onSuccess).
    const started = Date.now()
    setConfirm(null)
    setRunning({ group, done: 0, total: agents.length })
    turnOn.mutate(
      {
        ids: agents.map((a) => a.id),
        enabled: true,
        onProgress: (done) => setRunning((r) => (r ? { ...r, done } : r)),
      },
      {
        onSuccess: (res) => {
          // Until a read newer than the batch's start: its own refetch clears them if it succeeds.
          setHidden((h) => ({ ids: new Set([...h.ids, ...res.done]), until: started }))
          setFailed((f) => {
            const next = new Map(f)
            for (const id of res.done) next.delete(id)
            for (const x of res.failed) next.set(x.id, reasonOf(x.error))
            return next
          })
          announce(copy.attention.announce(res.done.length, res.failed.length))
        },
        onSettled: () => setRunning(null),
      },
    )
  }
  const ownerName = (id: string) => users.data?.get(id) ?? id.slice(0, 8)
  const pending = confirm === 'mine' ? mine : confirm === 'others' ? others : []

  return (
    <Strip>
      <div className="flex flex-col gap-3">
        <Header />
        {mine.length ? (
          <Line
            text={copy.attention.mine(mine.length, tiers.data?.compress_history === false)}
            agents={mine}
            action={copy.attention.turnOnMine(mine.length)}
            running={running?.group === 'mine' ? running : null}
            busy={running !== null}
            failed={failed}
            onAction={() => setConfirm('mine')}
            onRetry={(agents) => run('mine', agents)}
          />
        ) : null}
        {others.length ? (
          <Line
            text={copy.attention.others(others.length)}
            agents={others}
            action={copy.attention.turnOnAll(others.length)}
            running={running?.group === 'others' ? running : null}
            busy={running !== null}
            failed={failed}
            onAction={() => setConfirm('others')}
            onRetry={(agents) => run('others', agents)}
          />
        ) : null}
      </div>
      <ConfirmDialog
        open={confirm !== null}
        onOpenChange={(o) => {
          if (!o) setConfirm(null)
        }}
        title={copy.attention.confirmTitle(pending.length)}
        body={confirm === 'others' ? copy.attention.confirmOthers : copy.attention.confirmMine}
        confirmLabel={copy.attention.confirm(pending.length)}
        pending={false}
        error={null}
        onConfirm={() => confirm && run(confirm, pending)}
      >
        <ScrollArea className="[&>[data-slot=scroll-area-viewport]]:max-h-48">
          <ul className="m-0 flex list-none flex-col gap-1 p-0 text-sm">
            {pending.map((a) => (
              <li key={a.id}>
                {a.name}
                {confirm === 'others' ? (
                  <span className="text-muted-foreground">
                    {' · '}
                    {copy.attention.owner(ownerName(a.ownerId))}
                  </span>
                ) : null}
              </li>
            ))}
          </ul>
        </ScrollArea>
      </ConfirmDialog>
    </Strip>
  )
}

/** Every state keeps the block's h2 (R1B, R6x), the failed check included. */
function Header() {
  return (
    <div className="flex flex-wrap items-baseline gap-x-2">
      <h2 className="text-sm font-semibold">{copy.attention.title}</h2>
      <span className="text-xs text-muted-foreground">{copy.attention.now}</span>
    </div>
  )
}

function Strip({ children }: { children: ReactNode }) {
  return (
    <section aria-label={copy.attention.title}>
      <Alert className="border-warning/60">
        <AlertTriangle aria-hidden className="text-warning" />
        <AlertDescription className="text-foreground">{children}</AlertDescription>
      </Alert>
    </section>
  )
}

function Line({
  text,
  agents,
  action,
  running,
  busy,
  failed,
  onAction,
  onRetry,
}: {
  text: string
  agents: OffAgent[]
  action: string
  running: { done: number; total: number } | null
  busy: boolean
  failed: ReadonlyMap<string, string>
  onAction: () => void
  /** Failed agents were already confirmed, so Retry runs them again directly. */
  onRetry: (agents: OffAgent[]) => void
}) {
  const { shown, more } = shownNames(agents)
  const failures = agents.filter((a) => failed.has(a.id))
  return (
    <div className="flex flex-col gap-1">
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
        {/* A readable minimum width, so on a phone the button wraps below the line instead of squeezing it (QA ISSUE-005). */}
        <p className="m-0 min-w-60 flex-1">
          {text}{' '}
          {shown.map((a, i) => (
            <span key={a.id}>
              {i ? ', ' : null}
              <Link
                to="/agents/$agentId"
                params={{ agentId: a.id }}
                className="underline underline-offset-4 hover:text-primary-text"
              >
                {a.name}
              </Link>
            </span>
          ))}
          {more ? ` ${copy.attention.more(more)}` : null}.
        </p>
        {running ? (
          <span className="inline-flex items-center gap-2 text-sm text-muted-foreground">
            <Spinner aria-hidden className="motion-reduce:animate-none" />
            {copy.attention.progress(running.done, running.total)}
          </span>
        ) : (
          <Button
            variant="outline"
            size="sm"
            className="pointer-coarse:min-h-11"
            disabled={busy}
            onClick={onAction}
          >
            {action}
          </Button>
        )}
      </div>
      {failures.length && !running ? (
        <div className="flex flex-wrap items-start gap-x-3 gap-y-1">
          <ul className="m-0 flex list-none flex-col gap-0.5 p-0 text-xs text-destructive">
            {failures.map((a) => (
              <li key={a.id}>
                {copy.attention.failed(a.name, failed.get(a.id) ?? copy.attention.failedFallback)}
              </li>
            ))}
          </ul>
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0 text-xs pointer-coarse:min-h-11"
            disabled={busy}
            onClick={() => onRetry(failures)}
          >
            {copy.attention.retryFailed}
          </Button>
        </div>
      ) : null}
    </div>
  )
}
