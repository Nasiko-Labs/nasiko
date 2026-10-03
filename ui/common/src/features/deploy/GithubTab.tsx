/**
 * From GitHub (plans/feat-deploy.md §4.2; design review 6): the connection, a searchable repository list with one
 * selectable row (`?repo=` in the URL), branch and name, then `POST /api/github/clone` → the build page. A version clash
 * is only known after the clone (VERSION_CONFLICT in the upload row); the build page's "Deploy as vX" comes back here
 * with `?version=`, sent as `version_override`.
 */
import { Link, useNavigate } from '@tanstack/react-router'
import { AlertTriangle, ExternalLink, FolderGit2, Lock, SearchX } from 'lucide-react'
import { useId, useMemo, useState } from 'react'
import { CopyButton } from '@/components/shared/copy-button'
import { PageLoader } from '@/components/shared/page-loader'
import { PanelError } from '@/components/shared/panel'
import { SearchInput } from '@/components/shared/search-input'
import { EmptyState, StateCard } from '@/components/shared/state-card'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Skeleton } from '@/components/ui/skeleton'
import { relTime } from '@/features/agents/format'
import { ApiError } from '@/lib/api/client'
import {
  useGithubClone,
  useGithubConfigured,
  useGithubLogout,
  useGithubRepos,
  useGithubUser,
} from './api'
import { copy } from './copy'
import type { DeployStarted } from './UploadTab'
import { filterRepos, useGithubConnect } from './github'
import { nameFromFile, nameProblem, uploadCommand } from './name'
import type { DeploySearch } from './search'
import { Requirements } from './components/Requirements'
import { parseVersion } from './version'

const RULES = {
  dockerfile: { state: 'unknown' },
  entrypoint: { state: 'unknown' },
  version: { state: 'unknown' },
  size: { state: 'unknown' },
} as const

/** Only a plain GitHub repository URL goes into the copied CLI line (it's pasted into a terminal). */
const GITHUB_REPO_URL = /^https:\/\/github\.com\/[\w.-]+\/[\w.-]+$/
export function GithubTab({
  search,
  setSearch,
  onStarted,
}: {
  search: DeploySearch
  setSearch: (patch: Partial<DeploySearch>) => void
  onStarted?: (to: DeployStarted) => void
}) {
  const configured = useGithubConfigured()
  const on = configured.data === true
  const user = useGithubUser(on)
  const connected = !!user.data?.connected && user.data.valid !== false
  const repos = useGithubRepos(on && connected)
  const { state: connectState, connect } = useGithubConnect()
  const logout = useGithubLogout()

  if (configured.isPending)
    return <PageLoader label={copy.github.loading} inline className="min-h-64" />
  if (configured.isError)
    return (
      <PanelError
        error={configured.error}
        onRetry={() => void configured.refetch()}
        what={copy.github.what}
      />
    )
  if (!on) {
    return (
      <StateCard
        title={copy.github.notConfigured}
        action={
          <Button asChild size="sm" variant="outline" className="pointer-coarse:min-h-11">
            <Link to="/deploy" search={{ method: 'upload' }}>
              {copy.github.useUpload}
            </Link>
          </Button>
        }
      >
        {copy.github.notConfiguredHint}
      </StateCard>
    )
  }

  const login = user.data?.login ?? ''
  const connection = user.isPending ? (
    <Skeleton className="h-9 w-64" />
  ) : connected ? (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <span>{copy.github.connectedAs(login)}</span>
      <Button
        type="button"
        variant="link"
        size="sm"
        className="h-auto px-0 pointer-coarse:min-h-11"
        disabled={logout.isPending}
        onClick={() => logout.mutate(undefined, { onSuccess: () => void connect() })}
      >
        {copy.github.switchAccount}
      </Button>
    </div>
  ) : (
    <div className="flex flex-col items-start gap-2">
      {user.data?.connected && user.data.valid === false ? (
        <p className="text-sm text-warning">{copy.github.invalid}</p>
      ) : (
        <p className="text-sm text-muted-foreground">{copy.github.connectHint}</p>
      )}
      <Button
        type="button"
        className="pointer-coarse:min-h-11"
        disabled={connectState.kind === 'waiting'}
        onClick={() => void connect()}
      >
        {user.data?.connected ? copy.github.reconnect : copy.github.connect}
      </Button>
      {connectState.kind === 'waiting' ? (
        <p className="text-xs text-muted-foreground" role="status">
          {copy.github.waiting}
        </p>
      ) : null}
      {connectState.kind === 'failed' ? (
        <p className="text-xs text-destructive" role="alert">
          {copy.github.connectFailed}
        </p>
      ) : null}
      {connectState.kind === 'blocked' ? (
        <p className="text-xs" role="alert">
          {copy.github.blocked}{' '}
          <a
            href={connectState.url}
            target="_blank"
            rel="noopener noreferrer"
            className="text-primary-text underline underline-offset-4"
          >
            {copy.github.openHere} <ExternalLink aria-hidden className="inline size-3" />
          </a>
          .
        </p>
      ) : null}
    </div>
  )

  return (
    <div className="@container/github flex flex-col gap-4">
      <div className="grid grid-cols-1 gap-4 @[768px]/github:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] @[768px]/github:items-start">
        <Card className="gap-4 p-4">
          {connection}
          {connected ? (
            <RepoPicker
              search={search}
              setSearch={setSearch}
              repos={repos}
              login={login}
              onStarted={onStarted}
            />
          ) : null}
        </Card>
        <div className="@max-[768px]/github:order-first">
          <Requirements
            items={RULES}
            advisory={false}
            title={copy.github.needs}
            note={copy.github.needsNote}
          />
        </div>
      </div>
    </div>
  )
}

function RepoPicker({
  search,
  setSearch,
  repos,
  login,
  onStarted,
}: {
  search: DeploySearch
  setSearch: (patch: Partial<DeploySearch>) => void
  repos: ReturnType<typeof useGithubRepos>
  login: string
  onStarted?: (to: DeployStarted) => void
}) {
  const ids = useId()
  const navigate = useNavigate()
  const clone = useGithubClone()
  const [q, setQ] = useState('')
  const all = useMemo(() => repos.data ?? [], [repos.data])
  const shown = useMemo(() => filterRepos(all, q), [all, q])
  const picked = all.find((r) => r.full_name === search.repo) ?? null
  // Typed values win; otherwise the picked repository's default branch and name (never overwriting what was typed).
  const [typedBranch, setTypedBranch] = useState<string | null>(null)
  const [typedName, setTypedName] = useState<string | null>(search.name ?? null)
  const branch = typedBranch ?? picked?.default_branch ?? ''
  const name = typedName ?? (picked ? nameFromFile(picked.name) : '')
  const [touched, setTouched] = useState({ branch: false, name: !!search.name })
  const [pickError, setPickError] = useState(false)

  const nameErr = touched.name || clone.isError ? nameProblem(name) : null
  const override = search.version && parseVersion(search.version) ? search.version : undefined
  const err = clone.error instanceof ApiError ? clone.error : null
  const errText = err && typeof err.body === 'string' ? err.body : err?.message
  const branchErr = err?.status === 422 && /invalid request/i.test(errText ?? '') ? errText : null
  const serverNameErr = err?.status === 400 ? errText : null

  const submit = () => {
    if (!picked) {
      setPickError(true)
      document.getElementById(`${ids}-repos`)?.focus()
      return
    }
    setTouched({ branch: true, name: true })
    // Say what's missing and go there (never a red border alone).
    if (!branch.trim()) return void document.getElementById(`${ids}-branch`)?.focus()
    if (nameProblem(name)) return void document.getElementById(`${ids}-name`)?.focus()
    clone.mutate(
      {
        repository_full_name: picked.full_name,
        branch: branch.trim(),
        agent_name: name,
        ...(override ? { version_override: override } : {}),
      },
      {
        onSuccess: (buildId) =>
          onStarted
            ? onStarted({ buildId })
            : void navigate({ to: '/builds/$buildId', params: { buildId } }),
      },
    )
  }

  if (repos.isPending)
    return <PageLoader label={copy.github.loadingRepos} inline className="min-h-64" />
  if (repos.isError)
    return (
      <PanelError
        error={repos.error}
        onRetry={() => void repos.refetch()}
        what={copy.github.what}
      />
    )
  if (!all.length)
    return (
      <EmptyState
        icon={FolderGit2}
        title={copy.github.noRepos(login)}
        action={
          <Button asChild size="sm" variant="outline" className="pointer-coarse:min-h-11">
            <Link to="/deploy" search={{ method: 'upload' }}>
              {copy.github.useUpload}
            </Link>
          </Button>
        }
      >
        {copy.github.noReposHint}
      </EmptyState>
    )

  return (
    <form
      noValidate
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault()
        submit()
      }}
    >
      <SearchInput
        aria-label={copy.github.search}
        placeholder={copy.github.searchPlaceholder}
        value={q}
        onChange={(e) => setQ(e.target.value)}
      />
      {shown.length ? (
        <ScrollArea className="rounded-md border [&>[data-slot=scroll-area-viewport]]:max-h-72 [&>[data-slot=scroll-area-viewport]>div]:block!">
          <RadioGroup
            id={`${ids}-repos`}
            aria-label={copy.github.reposLabel}
            value={search.repo ?? ''}
            // A new repository starts from its own branch and name: what was typed for the last one doesn't carry over.
            onValueChange={(v) => {
              setPickError(false)
              clone.reset()
              setTypedBranch(null)
              setTypedName(null)
              setTouched({ branch: false, name: false })
              setSearch({ repo: v, version: undefined })
            }}
            className="gap-0 divide-y"
          >
            {shown.map((r) => (
              <label
                key={r.id}
                htmlFor={`${ids}-${r.id}`}
                className="flex cursor-pointer items-start gap-3 px-3 py-2.5 hover:bg-accent/50 has-[[data-state=checked]]:bg-accent pointer-coarse:min-h-11"
                data-testid="repo-row"
              >
                <RadioGroupItem id={`${ids}-${r.id}`} value={r.full_name} className="mt-0.5" />
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-1.5 text-sm font-medium">
                    <span className="truncate">{r.full_name}</span>
                    {r.private ? (
                      <Lock
                        aria-label={copy.github.private}
                        className="size-3.5 shrink-0 text-muted-foreground"
                      />
                    ) : null}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {r.description ? `${r.description} · ` : ''}
                    {copy.github.updated(relTime(r.updated_at))}
                  </span>
                </span>
              </label>
            ))}
          </RadioGroup>
        </ScrollArea>
      ) : (
        <EmptyState
          icon={SearchX}
          title={copy.github.noMatch(q)}
          action={
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="pointer-coarse:min-h-11"
              onClick={() => setQ('')}
            >
              {copy.github.clearSearch}
            </Button>
          }
        >
          {copy.github.noMatchHint}
        </EmptyState>
      )}
      <p className="text-xs text-muted-foreground">{copy.github.shown(all.length)}</p>
      {pickError ? (
        <p className="text-xs text-destructive" role="alert">
          {copy.github.pickFirst}
        </p>
      ) : null}

      {picked ? (
        <>
          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-branch`}>{copy.github.branch}</FieldLabel>
            <Input
              id={`${ids}-branch`}
              value={branch}
              placeholder={picked.default_branch}
              autoComplete="off"
              spellCheck={false}
              onChange={(e) => {
                setTypedBranch(e.target.value)
                setTouched((t) => ({ ...t, branch: true }))
              }}
              aria-invalid={!!branchErr || (touched.branch && !branch.trim())}
              aria-describedby={`${ids}-branch-msg`}
            />
            {branchErr || (touched.branch && !branch.trim()) ? (
              <p id={`${ids}-branch-msg`} className="text-xs text-destructive">
                {branchErr ?? copy.github.branchRequired}
              </p>
            ) : (
              <FieldDescription id={`${ids}-branch-msg`} className="text-xs">
                {copy.github.branchHint(picked.default_branch)}
              </FieldDescription>
            )}
          </Field>
          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-name`}>{copy.deploy.name}</FieldLabel>
            <Input
              id={`${ids}-name`}
              value={name}
              placeholder={copy.deploy.namePlaceholder}
              autoComplete="off"
              spellCheck={false}
              onChange={(e) => {
                setTypedName(e.target.value)
                setTouched((t) => ({ ...t, name: true }))
              }}
              aria-invalid={!!(nameErr || serverNameErr)}
              aria-describedby={`${ids}-name-msg`}
            />
            {nameErr || serverNameErr ? (
              <p id={`${ids}-name-msg`} className="text-xs text-destructive">
                {nameErr ?? serverNameErr}
              </p>
            ) : (
              <FieldDescription id={`${ids}-name-msg`} className="text-xs">
                {copy.deploy.nameHint}
              </FieldDescription>
            )}
          </Field>
          {override ? (
            <p className="text-xs text-muted-foreground">
              {copy.deploy.version}: {override}
            </p>
          ) : null}
        </>
      ) : null}

      {err && !branchErr && !serverNameErr ? (
        <Alert className="gap-y-1 border-destructive/30 bg-destructive/5 [&>svg]:text-destructive">
          <AlertTriangle aria-hidden />
          <AlertTitle className="line-clamp-none">{copy.deploy.failed}</AlertTitle>
          <AlertDescription>
            {err.status === 403 ? copy.errors.githubDisconnected.problem : errText}
          </AlertDescription>
        </Alert>
      ) : null}

      <Button type="submit" className="w-full pointer-coarse:min-h-11" disabled={clone.isPending}>
        <span className="truncate">{copy.github.submit(picked?.full_name ?? '')}</span>
      </Button>
      {picked && GITHUB_REPO_URL.test(picked.html_url) ? (
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground @max-[768px]/github:hidden">
          <span>{copy.deploy.terminal}</span>
          <CopyButton
            text={`git clone ${picked.html_url} && ${uploadCommand(picked.name)}`}
            label={copy.deploy.copyCommand}
            showText
          />
        </div>
      ) : null}
    </form>
  )
}
