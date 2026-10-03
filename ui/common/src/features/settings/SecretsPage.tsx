/**
 * Secrets (plans/feat-settings.md §1.2), as nasiko-cloud-rs `43833316` ui/common/pages/secrets-page.js and
 * features/secrets-manager.js (scope "user") work: every signed-in user's own secrets, by name.
 * - Rows: the name, the value masked, "Updated 3d ago"; Show / Copy read the value on an explicit click
 *   (`GET /api/secrets/{name}`, the caller's own only), and a shown value masks itself again after 30 s, on any
 *   refresh (someone may have rewritten it) and when the page closes.
 * - Delete asks inline, and says which of your router configs lose their key (the server doesn't check, ST-7).
 * - Add is an inline row: `POST` upserts, so an existing name gets the new value.
 */
import { zodResolver } from '@hookform/resolvers/zod'
import { useQuery } from '@tanstack/react-query'
import { Copy, Eye, EyeOff, KeyRound, Lock, Plus, Trash2 } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { useForm } from 'react-hook-form'
import { toast } from 'sonner'
import { z } from 'zod'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { PanelError } from '@/components/shared/panel'
import { EmptyState } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Field, FieldError, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { useConfigs, useSecrets } from '@/features/router/api'
import { meQuery } from '@/lib/api/auth'
import { ApiError } from '@/lib/api/client'
import { useNow } from '@/lib/useNow'
import { useSecretWrites } from './api'
import { copy as all } from './copy'
import { ago, configsUsing, secretNameProblem } from './logic'

const copy = all.secrets

/** How long a shown value stays on screen (legacy AUTO_REMASK_MS). */
export const REMASK_MS = 30_000

const reason = (err: unknown) =>
  (err instanceof ApiError && err.serverMessage) || (err as Error).message

export function SecretsPage() {
  const me = useQuery(meQuery)
  const secrets = useSecrets()
  // Only for "Used by" in the delete prompt; a member's own configs (the list is the caller's).
  const configs = useConfigs(!!me.data)
  const w = useSecretWrites()
  const now = useNow(60_000)
  const [confirming, setConfirming] = useState<string | null>(null)
  // name → plaintext, only for rows the user chose to show. Never in the query cache.
  const [shown, setShown] = useState<ReadonlyMap<string, string>>(new Map())
  const timers = useRef(new Map<string, ReturnType<typeof setTimeout>>())
  const mask = (name: string) => {
    clearTimeout(timers.current.get(name))
    timers.current.delete(name)
    setShown((m) => {
      if (!m.has(name)) return m
      const next = new Map(m)
      next.delete(name)
      return next
    })
  }
  // A re-listed secret may have been rewritten elsewhere: showing the old plaintext beside a new "Updated" would lie.
  const listedAt = secrets.dataUpdatedAt
  useEffect(() => {
    const t = timers.current
    return () => {
      for (const id of t.values()) clearTimeout(id)
      t.clear()
      setShown(new Map())
    }
  }, [listedAt])

  const reveal = (name: string) => {
    if (shown.has(name)) return mask(name)
    if (w.read.isPending) return
    w.read.mutate(name, {
      onSuccess: (value) => {
        setShown((m) => new Map(m).set(name, value))
        timers.current.set(
          name,
          setTimeout(() => mask(name), REMASK_MS),
        )
      },
      onError: (err) => toast.error(copy.readFailed(name, reason(err))),
      onSettled: () => w.read.reset(),
    })
  }
  const copyValue = async (name: string) => {
    // Undefined outside a secure context (a self-hosted server on plain HTTP): say why.
    if (!navigator.clipboard?.writeText) return void toast.error(copy.needsSecureContext)
    try {
      const value = shown.get(name) ?? (await w.read.mutateAsync(name))
      await navigator.clipboard.writeText(value)
      toast.success(copy.copied(name))
    } catch (err) {
      toast.error(copy.copyFailed(name, reason(err)))
    } finally {
      w.read.reset()
    }
  }
  const remove = (name: string) =>
    w.remove.mutate(name, {
      onSuccess: () => {
        toast.success(copy.deleted(name))
        setConfirming(null)
        mask(name)
      },
      onError: (err) => toast.error(copy.deleteFailed(name, reason(err))),
    })

  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={copy.title} description={copy.sub} />
      {secrets.isPending ? (
        <PageLoader label={copy.loading} />
      ) : secrets.isError ? (
        <PanelError
          error={secrets.error}
          what={copy.loadWhat}
          onRetry={() => void secrets.refetch()}
        />
      ) : (
        // One card: the secrets, then the add row on its muted footer strip.
        <Card className="@container gap-0 overflow-hidden py-0">
          {secrets.data.length === 0 ? (
            <EmptyState icon={KeyRound} title={copy.empty} className="rounded-none border-0">
              {copy.emptyText}
            </EmptyState>
          ) : (
            <ul className="divide-y divide-border">
              {secrets.data.map((s) => {
                const value = shown.get(s.name)
                const pending = w.read.isPending && w.read.variables === s.name
                if (confirming === s.name) {
                  const used = configsUsing(configs.data, s.name)
                  return (
                    <li key={s.name} className="flex flex-wrap items-center gap-3 px-4 py-3">
                      <Name name={s.name} />
                      <span className="text-sm">
                        {copy.confirm}
                        {used.length ? (
                          <span className="text-muted-foreground">
                            {' '}
                            {copy.usedBy(used.join(', '))}
                          </span>
                        ) : null}
                      </span>
                      <span className="ml-auto flex gap-2">
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => setConfirming(null)}
                          disabled={w.remove.isPending}
                        >
                          {copy.cancel}
                        </Button>
                        <Button
                          size="sm"
                          variant="destructive"
                          onClick={() => remove(s.name)}
                          disabled={w.remove.isPending}
                        >
                          {copy.confirmDelete}
                        </Button>
                      </span>
                    </li>
                  )
                }
                return (
                  <li
                    key={s.name}
                    className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 px-4 py-3 @[640px]:grid-cols-[minmax(0,14rem)_minmax(0,1fr)_auto_auto]"
                  >
                    <Name name={s.name} />
                    <span
                      className="order-3 col-span-2 truncate font-mono text-xs text-muted-foreground @[640px]:order-none @[640px]:col-span-1"
                      title={value}
                    >
                      {value ?? copy.masked}
                    </span>
                    <span className="order-4 text-xs text-muted-foreground @[640px]:order-none">
                      {copy.updated(ago(s.updated_at || s.created_at, now))}
                    </span>
                    <span className="flex justify-end gap-1">
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-pressed={value !== undefined}
                        aria-label={value !== undefined ? copy.hide(s.name) : copy.show(s.name)}
                        disabled={pending}
                        onClick={() => reveal(s.name)}
                      >
                        {value !== undefined ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
                      </Button>
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label={copy.copy(s.name)}
                        onClick={() => void copyValue(s.name)}
                      >
                        <Copy aria-hidden />
                      </Button>
                      <Button
                        size="icon"
                        variant="ghost"
                        aria-label={copy.delete(s.name)}
                        onClick={() => setConfirming(s.name)}
                      >
                        <Trash2 aria-hidden />
                      </Button>
                    </span>
                  </li>
                )
              })}
            </ul>
          )}
          <div className="border-t border-border bg-muted/40 px-4 py-4">
            <AddSecret />
          </div>
        </Card>
      )}
      <p className="flex items-start gap-2 px-1 text-xs text-muted-foreground">
        <Lock aria-hidden className="mt-px size-3.5 shrink-0" />
        {copy.note}
      </p>
    </div>
  )
}

function Name({ name }: { name: string }) {
  return (
    <span className="flex min-w-0 items-center gap-2 text-sm">
      <Lock aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
      <span className="truncate">{name}</span>
    </span>
  )
}

const schema = z.object({
  name: z
    .string()
    .trim()
    .superRefine((n, ctx) => {
      const p = secretNameProblem(n)
      if (p) ctx.addIssue({ code: 'custom', message: copy.nameProblem[p] })
    }),
  value: z.string().min(1, copy.valueRequired),
})

/** The inline add row. A fresh form after each save, not reset() (plans/feat-settings.md §3). */
function AddSecret() {
  const [round, setRound] = useState(0)
  return <AddSecretForm key={round} onSaved={() => setRound((r) => r + 1)} />
}

function AddSecretForm({ onSaved }: { onSaved: () => void }) {
  const id = useId()
  const { add } = useSecretWrites()
  const form = useForm({ resolver: zodResolver(schema), defaultValues: { name: '', value: '' } })
  const { errors } = form.formState
  const onSubmit = form.handleSubmit((v) =>
    add.mutate(v, {
      onSuccess: () => {
        toast.success(copy.saved(v.name))
        onSaved()
      },
      onError: (err) => toast.error(copy.saveFailed(v.name, reason(err))),
      // The value must not linger in the mutation's cached variables.
      onSettled: () => add.reset(),
    }),
  )
  return (
    <form
      onSubmit={(e) => void onSubmit(e)}
      noValidate
      className="grid items-start gap-3 @[640px]:grid-cols-[minmax(0,14rem)_minmax(0,1fr)_auto]"
    >
      <Field data-invalid={!!errors.name} className="gap-1.5">
        <FieldLabel htmlFor={`${id}-name`}>{copy.name}</FieldLabel>
        <Input
          id={`${id}-name`}
          placeholder={copy.namePlaceholder}
          maxLength={128}
          autoComplete="off"
          spellCheck={false}
          autoCapitalize="characters"
          aria-invalid={!!errors.name}
          {...form.register('name')}
        />
        <FieldError errors={[errors.name]} />
      </Field>
      <Field data-invalid={!!errors.value} className="gap-1.5">
        <FieldLabel htmlFor={`${id}-value`}>{copy.value}</FieldLabel>
        <Input
          id={`${id}-value`}
          type="password"
          placeholder={copy.valuePlaceholder}
          autoComplete="off"
          aria-invalid={!!errors.value}
          {...form.register('value')}
        />
        <FieldError errors={[errors.value]} />
      </Field>
      {/* Level with the inputs (same h-9): pushed down by the label row, text-sm × leading-snug plus the field's gap-1.5. */}
      <Button
        type="submit"
        className="@[640px]:mt-[calc(0.875rem*1.375+0.375rem)]"
        disabled={add.isPending}
      >
        <Plus aria-hidden /> {add.isPending ? copy.adding : copy.add}
      </Button>
    </form>
  )
}
