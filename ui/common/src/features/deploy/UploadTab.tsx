/**
 * Upload a zip (plans/feat-deploy.md §4.1). The zip is checked in the browser before anything is sent (R5); the upload
 * runs in the registry (R1, R2), so moving around the app keeps it going and this form re-attaches to it.
 *
 * States (design review 4): no file → neutral checklist, Deploy says why on click; uploading → a labelled progress bar
 * with Cancel, fields read-only; rejected → the error lands on the checklist item or field it names.
 */
import { zodResolver } from '@hookform/resolvers/zod'
import { useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { AlertTriangle, Plus, SquareTerminal, X } from 'lucide-react'
import { useEffect, useId, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { useFieldArray, useForm, useWatch } from 'react-hook-form'
import { z } from 'zod'
import { CopyButton } from '@/components/shared/copy-button'
import { Disclosure } from '@/components/shared/disclosure'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Progress } from '@/components/ui/progress'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { useAgentsDirectory } from '@/features/agents/api'
import { fmtBytes } from '@/lib/format'
import { copy } from './copy'
import { explainError } from './errors'
import { nameFromFile, nameProblem, uploadCommand } from './name'
import {
  cancelUpload,
  dismissUpload,
  startUpload,
  subscribeUploads,
  uploadFor,
  type UploadFields,
} from './uploads'
import { nextPatch, parseVersion } from './version'
import {
  CHECKLIST_ITEMS as ITEMS,
  checkZip,
  MAX_ZIP_BYTES,
  type ItemResults,
  type ZipCheck,
} from './zipcheck'
import { DropZone } from './components/DropZone'
import { Requirements, RequirementsCompact } from './components/Requirements'

const INBOUND = ['openai', 'anthropic', 'gemini'] as const

const schema = z.object({
  name: z.string().superRefine((v, ctx) => {
    const p = nameProblem(v)
    if (p) ctx.addIssue({ code: 'custom', message: p })
  }),
  version: z
    .string()
    .trim()
    .refine((v) => v === '' || !!parseVersion(v), copy.deploy.versionFormat),
  // 1–65535 each: the server parses u16 and silently drops anything else (falling back to 8000).
  ports: z
    .string()
    .trim()
    .refine(
      (v) =>
        v === '' ||
        (/^\d{1,5}(\s*,\s*\d{1,5})*$/.test(v) &&
          v.split(',').every((p) => Number(p) >= 1 && Number(p) <= 65535)),
      copy.deploy.portsFormat,
    ),
  env: z.array(z.object({ key: z.string(), value: z.string() })),
  storageTouched: z.boolean(),
  writable: z.boolean(),
  writablePath: z.string().trim(),
  inbound: z.enum(['default', ...INBOUND]),
})
type Form = z.infer<typeof schema>

const EMPTY_ITEMS: ItemResults = {
  dockerfile: { state: 'unknown' },
  entrypoint: { state: 'unknown' },
  version: { state: 'unknown' },
  size: { state: 'unknown' },
}

/** Where a started deploy goes: the Build page, or the agent when an import didn't build. The onboarding guide passes
 *  its own handler to stay open (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §3). */
export type DeployStarted = { buildId: string } | { agentId: string }

export function UploadTab({
  userId,
  prefill,
  onStarted,
}: {
  userId: string
  prefill: { name?: string; version?: string }
  onStarted?: (to: DeployStarted) => void
}) {
  const ids = useId()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const upload = useSyncExternalStore(subscribeUploads, () => uploadFor(userId))
  const busy = upload?.phase === 'uploading'
  const [file, setFile] = useState<File | null>(null)
  const [check, setCheck] = useState<ZipCheck | null>(null)
  const [checking, setChecking] = useState(false)
  const latestFile = useRef<File | null>(null)
  const [fileError, setFileError] = useState<string | null>(null)
  const [blocked, setBlocked] = useState(false)
  // Bumped on each blocked Deploy: opens the phone checklist, then focuses its failing row once it's rendered.
  const [blockTick, setBlockTick] = useState(0)
  const chooseRef = useRef<HTMLButtonElement>(null)
  const dir = useAgentsDirectory()

  const {
    control,
    register,
    setValue,
    handleSubmit,
    setError,
    setFocus,
    getFieldState,
    formState,
  } = useForm<Form>({
    resolver: zodResolver(schema),
    defaultValues: {
      name: prefill.name ?? '',
      version: prefill.version ?? '',
      ports: '',
      env: [],
      storageTouched: false,
      writable: false,
      writablePath: '',
      inbound: 'default',
    },
    mode: 'onTouched',
  })
  const env = useFieldArray({ control, name: 'env' })
  const [name, version, writable] = useWatch({ control, name: ['name', 'version', 'writable'] })
  const [advancedOpen, setAdvancedOpen] = useState(false)

  // The upload finished here or while the user was elsewhere: open its build (push; design review 4).
  useEffect(() => {
    if (upload?.phase !== 'done') return
    dismissUpload(userId)
    if (onStarted) onStarted({ buildId: upload.buildId })
    else void navigate({ to: '/builds/$buildId', params: { buildId: upload.buildId } })
  }, [upload, userId, navigate, onStarted])

  // An agent with this name that the viewer can see: its current version and the next patch (a 409 caught early).
  const existing = useMemo(
    () => (name ? (dir.byNameAll.get(name) ?? [])[0] : undefined),
    [dir.byNameAll, name],
  )
  const suggested = existing ? nextPatch(existing.version) : null

  const serverError = upload?.phase === 'failed' ? upload.error : null
  // Stable per error: it feeds the focus effect below, which must run once per rejection, not on every keystroke.
  const explained = useMemo(
    () =>
      serverError
        ? explainError(typeof serverError.body === 'string' ? serverError.body : null)
        : null,
    [serverError],
  )
  const items: ItemResults = useMemo(() => {
    const base = check?.items ?? EMPTY_ITEMS
    const withVersion = {
      ...base,
      version: version && parseVersion(version) ? { state: 'pass' as const } : base.version,
    }
    if (explained?.item && explained.item !== 'version')
      return { ...withVersion, [explained.item]: { state: 'fail', detail: explained.fix } }
    return withVersion
  }, [check, version, explained])

  useEffect(() => {
    if (blockTick) focusItem(ITEMS.find((i) => items[i].state === 'fail'))
    // Once per blocked Deploy.
  }, [blockTick]) // eslint-disable-line react-hooks/exhaustive-deps

  // Focus what the server named (design review 4).
  useEffect(() => {
    if (!serverError || !explained) return
    if (serverError.status === 409 || explained.item === 'version') setFocus('version')
    else if (explained.item) focusItem(explained.item)
    // Once per rejection: `explained` and `setFocus` follow `serverError`.
  }, [serverError]) // eslint-disable-line react-hooks/exhaustive-deps

  const onFile = async (f: File) => {
    setBlocked(false)
    dismissUpload(userId)
    if (!/\.zip$/i.test(f.name)) {
      setFileError(copy.deploy.zipOnly)
      return
    }
    setFileError(f.size > MAX_ZIP_BYTES ? copy.errors.tooLarge.problem : null)
    setFile(f)
    setChecking(true)
    latestFile.current = f
    const r = await checkZip(f)
    // A newer file was picked while this one was read: its result wins.
    if (latestFile.current !== f) return
    setCheck(r)
    setChecking(false)
    // Never overwrite what the user typed.
    if (!getFieldState('name').isDirty)
      setValue('name', nameFromFile(r.cardName ?? '') || nameFromFile(f.name), {
        shouldValidate: true,
      })
    if (!getFieldState('version').isDirty && r.version)
      setValue('version', r.version, { shouldValidate: true })
  }

  // Only reached with a file: onSubmit below handles "no file yet" before field validation.
  const submit = (v: Form) => {
    if (!file) return
    if (ITEMS.some((i) => items[i].state === 'fail' && i !== 'version')) {
      setBlocked(true)
      setBlockTick((t) => t + 1)
      return
    }
    if (!v.version && !check?.version) {
      setError('version', { message: copy.deploy.versionNeeded })
      setFocus('version')
      return
    }
    setBlocked(false)
    const envObj = Object.fromEntries(
      v.env.filter((e) => e.key.trim()).map((e) => [e.key.trim(), e.value]),
    )
    const fields: UploadFields = {
      name: v.name,
      version: v.version || undefined,
      ports: v.ports || undefined,
      env: envObj,
      // Tri-state on the server: sent only when touched.
      writable: v.storageTouched ? v.writable : undefined,
      writablePath: v.storageTouched && v.writable ? v.writablePath || undefined : undefined,
      inboundFormat: v.inbound === 'default' ? undefined : v.inbound,
    }
    void startUpload(qc, userId, file, file.name, fields)
  }

  const err = (k: keyof Form) => (formState.errors[k] as { message?: string } | undefined)?.message
  const versionErr =
    err('version') ?? (serverError?.status === 409 ? explained?.problem : undefined)
  const conflictNext = serverError?.status === 409 ? (explained?.suggested ?? suggested) : null
  const other =
    serverError && serverError.status !== 409 && serverError.status !== 413 && !explained?.item
      ? serverError
      : null

  return (
    <div className="@container/deploy flex flex-col gap-4">
      <form
        noValidate
        onSubmit={(e) => {
          // No file yet: say so before any field validation (design review 4).
          if (!file) {
            e.preventDefault()
            setFileError(copy.deploy.chooseFirst)
            chooseRef.current?.focus()
            return
          }
          void handleSubmit(submit)(e)
        }}
        className="@container/deploy grid grid-cols-1 gap-4 @[768px]/deploy:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] @[768px]/deploy:items-start"
      >
        <div className="@[768px]/deploy:hidden">
          <RequirementsCompact
            items={items}
            advisory={!!check && !check.readable}
            openSignal={blockTick}
          />
        </div>
        <Card className="gap-4 p-4">
          <DropZone
            ref={chooseRef}
            file={file}
            disabled={busy}
            error={fileError ?? (serverError?.status === 413 ? copy.errors.tooLarge.problem : null)}
            onFile={(f) => void onFile(f)}
            onClear={() => {
              latestFile.current = null
              setChecking(false)
              setFile(null)
              setCheck(null)
              setFileError(null)
              dismissUpload(userId)
            }}
          />
          {checking ? (
            <p className="text-xs text-muted-foreground" role="status">
              {copy.deploy.checking}
            </p>
          ) : null}

          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-name`}>{copy.deploy.name}</FieldLabel>
            <Input
              id={`${ids}-name`}
              readOnly={busy}
              placeholder={copy.deploy.namePlaceholder}
              autoComplete="off"
              spellCheck={false}
              {...register('name')}
              aria-invalid={!!err('name')}
              aria-describedby={err('name') ? `${ids}-name-err` : `${ids}-name-hint`}
            />
            {err('name') ? (
              <p id={`${ids}-name-err`} className="text-xs text-destructive">
                {err('name')}
              </p>
            ) : (
              <FieldDescription id={`${ids}-name-hint`} className="text-xs">
                {copy.deploy.nameHint}
              </FieldDescription>
            )}
          </Field>

          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-version`}>{copy.deploy.version}</FieldLabel>
            <Input
              id={`${ids}-version`}
              readOnly={busy}
              inputMode="decimal"
              placeholder="1.0.0"
              autoComplete="off"
              {...register('version')}
              aria-invalid={!!versionErr}
              aria-describedby={`${ids}-version-hint`}
            />
            <div id={`${ids}-version-hint`} className="flex flex-wrap items-center gap-2 text-xs">
              {versionErr ? (
                <span className="text-destructive">{versionErr}</span>
              ) : existing && suggested ? (
                <span className="text-muted-foreground">
                  {copy.deploy.current(existing.version, suggested)}
                </span>
              ) : (
                <span className="text-muted-foreground">{copy.deploy.versionHint}</span>
              )}
              {(conflictNext ??
              (existing && suggested && version !== suggested ? suggested : null)) ? (
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className="h-7 pointer-coarse:min-h-11"
                  disabled={busy}
                  onClick={() => {
                    setValue('version', conflictNext ?? suggested ?? '', {
                      shouldDirty: true,
                      shouldValidate: true,
                    })
                    dismissUpload(userId)
                  }}
                >
                  {copy.deploy.use(conflictNext ?? suggested ?? '')}
                </Button>
              ) : null}
            </div>
          </Field>

          <Disclosure
            id={`${ids}-adv`}
            title={copy.deploy.advanced}
            open={advancedOpen}
            onToggle={() => setAdvancedOpen((o) => !o)}
          >
            <Field className="gap-1">
              <FieldLabel htmlFor={`${ids}-ports`}>{copy.deploy.ports}</FieldLabel>
              <Input
                id={`${ids}-ports`}
                readOnly={busy}
                placeholder="8000"
                {...register('ports')}
                aria-invalid={!!err('ports')}
                aria-describedby={`${ids}-ports-msg`}
              />
              {err('ports') ? (
                <p id={`${ids}-ports-msg`} className="text-xs text-destructive">
                  {err('ports')}
                </p>
              ) : (
                <FieldDescription id={`${ids}-ports-msg`} className="text-xs">
                  {copy.deploy.portsHint}
                </FieldDescription>
              )}
            </Field>
            <fieldset className="flex flex-col gap-2">
              <legend className="mb-1 text-sm font-medium">{copy.deploy.env}</legend>
              {env.fields.map((f, i) => (
                <div key={f.id} className="flex gap-2">
                  <Input
                    aria-label={`${copy.deploy.envKey} ${i + 1}`}
                    placeholder={copy.deploy.envKey}
                    readOnly={busy}
                    className="font-mono"
                    {...register(`env.${i}.key`)}
                  />
                  <Input
                    aria-label={`${copy.deploy.envValue} ${i + 1}`}
                    placeholder={copy.deploy.envValue}
                    readOnly={busy}
                    className="font-mono"
                    {...register(`env.${i}.value`)}
                  />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="shrink-0 pointer-coarse:size-11"
                    aria-label={copy.deploy.removeEnv(f.key)}
                    disabled={busy}
                    onClick={() => env.remove(i)}
                  >
                    <X className="size-4" aria-hidden />
                  </Button>
                </div>
              ))}
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="self-start pointer-coarse:min-h-11"
                disabled={busy}
                onClick={() => env.append({ key: '', value: '' })}
              >
                <Plus className="size-3.5" aria-hidden /> {copy.deploy.addEnv}
              </Button>
            </fieldset>
            <div className="flex items-center gap-3">
              <Switch
                id={`${ids}-storage`}
                checked={writable}
                disabled={busy}
                onCheckedChange={(on) => {
                  setValue('writable', on)
                  setValue('storageTouched', true)
                }}
              />
              <label htmlFor={`${ids}-storage`} className="text-sm font-medium">
                {copy.deploy.storage}
              </label>
            </div>
            {writable ? (
              <Field className="gap-1">
                <FieldLabel htmlFor={`${ids}-path`}>{copy.deploy.storagePath}</FieldLabel>
                <Input
                  id={`${ids}-path`}
                  readOnly={busy}
                  placeholder="/workspace"
                  {...register('writablePath')}
                />
                <FieldDescription className="text-xs">
                  {copy.deploy.storagePathHint}
                </FieldDescription>
              </Field>
            ) : null}
            <Field className="gap-1">
              <FieldLabel htmlFor={`${ids}-inbound`}>{copy.deploy.inbound}</FieldLabel>
              <Select
                defaultValue="default"
                disabled={busy}
                onValueChange={(v) => setValue('inbound', v as Form['inbound'])}
              >
                <SelectTrigger id={`${ids}-inbound`} className="w-full pointer-coarse:min-h-11">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="default">{copy.deploy.inboundDefault}</SelectItem>
                  {INBOUND.map((f) => (
                    <SelectItem key={f} value={f}>
                      {f}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          </Disclosure>

          {blocked ? (
            <p className="text-xs text-destructive" role="alert">
              {copy.deploy.fixFirst}
            </p>
          ) : null}
          {other ? (
            <Alert className="gap-y-1 border-destructive/30 bg-destructive/5 [&>svg]:text-destructive">
              <AlertTriangle aria-hidden />
              <AlertTitle className="line-clamp-none">{copy.deploy.failed}</AlertTitle>
              <AlertDescription>
                {explained && explained.problem !== copy.errors.generic.problem
                  ? explained.problem
                  : typeof other.body === 'string' && other.body
                    ? other.body
                    : other.message}
              </AlertDescription>
            </Alert>
          ) : null}
          {upload?.phase === 'cancelled' ? (
            <p className="text-xs text-muted-foreground" role="status">
              {copy.deploy.cancelled}
            </p>
          ) : null}

          {busy && upload?.phase === 'uploading' ? (
            <div className="flex items-center gap-3" data-testid="upload-progress">
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <Progress
                  value={upload.total ? Math.round((upload.loaded / upload.total) * 100) : 0}
                  aria-label={copy.deploy.uploading(
                    fmtBytes(upload.loaded),
                    fmtBytes(upload.total),
                  )}
                />
                <p className="text-xs text-muted-foreground tabular-nums">
                  {copy.deploy.uploading(fmtBytes(upload.loaded), fmtBytes(upload.total))}
                </p>
              </div>
              {/* Once the whole file is sent the server may already have queued the build: Cancel can't promise to stop it. */}
              <Button
                type="button"
                variant="outline"
                className="shrink-0 pointer-coarse:min-h-11"
                disabled={upload.total > 0 && upload.loaded >= upload.total}
                onClick={() => cancelUpload(userId)}
              >
                {copy.deploy.cancel}
              </Button>
            </div>
          ) : (
            <Button
              type="submit"
              className="w-full truncate pointer-coarse:min-h-11"
              disabled={checking}
            >
              <span className="truncate">{copy.deploy.submit(name)}</span>
            </Button>
          )}
        </Card>
        {/* Stays in view under the Deploy page's sticky method row while the form scrolls. */}
        <div className="sticky top-16 @max-[768px]/deploy:hidden">
          <Requirements items={items} advisory={!!check && !check.readable} />
        </div>
      </form>
      {/* The CLI equivalent for the name being deployed; hidden on phones (design review 15). */}
      <Card className="flex-row flex-wrap items-center gap-3 p-4 @max-[768px]/deploy:hidden">
        <SquareTerminal aria-hidden className="size-5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium">{copy.deploy.terminal}</p>
          <p className="text-xs text-muted-foreground">{copy.deploy.terminalHint}</p>
        </div>
        <CopyButton text={uploadCommand(name)} label={copy.deploy.copyCommand} showText />
      </Card>
    </div>
  )
}

/** Focus a checklist row in whichever list is shown: the card from 768 px, the compact list on phones. */
function focusItem(id: string | undefined) {
  if (!id) return
  const rows = [
    document.getElementById(`zip-item-${id}`),
    document.getElementById(`zip-item-compact-${id}`),
  ].filter((e) => e !== null)
  ;(rows.find((e) => e.getClientRects().length > 0) ?? rows[0])?.focus()
}
