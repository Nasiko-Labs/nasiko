/**
 * Settings → Workspace → MCP toolkits (`?section=mcp`): the Composio API key and which toolkits people can connect to.
 * Its own read and Save (`/api/settings/composio`), not part of the workspace form. Superuser-only like the rest of the
 * workspace sections. The key is write-only: the server says whether one is in effect and where it came from, never
 * the key.
 */
import { useId, useMemo, useState } from 'react'
import { toast } from 'sonner'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PanelError } from '@nasiko/ui/composites/panel'
import { Alert, AlertDescription, AlertTitle } from '@nasiko/ui/primitives/alert'
import { Button } from '@nasiko/ui/primitives/button'
import { Checkbox } from '@nasiko/ui/primitives/checkbox'
import { Input } from '@nasiko/ui/primitives/input'
import { Label } from '@nasiko/ui/primitives/label'
import { ApiError } from '@nasiko/data-access/client'
import { AgentMark } from '@nasiko/features/agents/components/bits'
import { useComposioSettings, useSaveComposio } from './api'
import { SettingRow, SettingRows } from './components/SettingRow'
import { copy } from './copy'
import { sameSet } from './logic'
import type { ComposioSettings, ComposioUpdate } from './types'

const c = copy.composio

export function ComposioSection() {
  const settings = useComposioSettings()
  // A saved form is replaced by a fresh one on the saved row, as the workspace form is (a reset() form stops
  // reaching its fields under the React Compiler).
  const [saves, setSaves] = useState(0)
  if (settings.isPending) return <PageLoader label={c.loading} />
  if (settings.isError)
    return (
      <PanelError
        error={settings.error}
        what={c.loadWhat}
        onRetry={() => void settings.refetch()}
      />
    )
  return (
    <ComposioForm key={saves} settings={settings.data} onSaved={() => setSaves((n) => n + 1)} />
  )
}

function ComposioForm({ settings, onSaved }: { settings: ComposioSettings; onSaved: () => void }) {
  const id = useId()
  const save = useSaveComposio()
  const enabled = useMemo(
    () => new Set(settings.toolkits.filter((t) => t.enabled).map((t) => t.toolkit)),
    [settings.toolkits],
  )
  const [keyDraft, setKeyDraft] = useState('')
  const [removeKey, setRemoveKey] = useState(false)
  const [picked, setPicked] = useState<ReadonlySet<string>>(enabled)
  const [search, setSearch] = useState('')

  const typed = keyDraft.trim()
  const toolkitsChanged = !sameSet(picked, enabled)
  const dirty = typed !== '' || removeKey || toolkitsChanged
  // Switching toolkits on needs a key to register them with. Removing the saved key may leave the server's own, which
  // only the server knows, so that case is left to it.
  const needsKey = !settings.api_key_configured && typed === '' && picked.size > 0
  const shown = settings.toolkits.filter((t) =>
    `${t.display_name} ${t.toolkit}`.toLowerCase().includes(search.trim().toLowerCase()),
  )

  const toggle = (toolkit: string, on: boolean) =>
    setPicked((prev) => {
      const next = new Set(prev)
      if (on) next.add(toolkit)
      else next.delete(toolkit)
      return next
    })
  const setShown = (on: boolean) =>
    setPicked((prev) => {
      const next = new Set(prev)
      for (const t of shown) {
        if (on) next.add(t.toolkit)
        else next.delete(t.toolkit)
      }
      return next
    })

  const onSubmit = (e: React.FormEvent) => {
    e.preventDefault()
    if (!dirty || needsKey) return
    const update: ComposioUpdate = {}
    if (typed) update.api_key = typed
    if (removeKey) update.clear_api_key = true
    if (toolkitsChanged) update.toolkits = [...picked]
    save.mutate(update, {
      onSuccess: (saved) => {
        if (saved.failed.length > 0) toast.warning(c.savedPartly(saved.failed.length))
        else toast.success(c.saved)
        onSaved()
      },
      onError: (err) =>
        toast.error(
          c.saveFailed((err instanceof ApiError && err.serverMessage) || (err as Error).message),
        ),
    })
  }

  const keyId = `${id}-key`
  return (
    <form data-tour="settings-mcp" onSubmit={onSubmit} noValidate className="flex flex-col gap-4">
      {settings.failed.length > 0 ? (
        <Alert variant="destructive">
          <AlertTitle>{c.failedTitle}</AlertTitle>
          <AlertDescription>
            <ul className="list-disc pl-4">
              {settings.failed.map((f) => (
                <li key={f.toolkit}>
                  {f.toolkit}: {f.error}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      ) : null}
      <SettingRows
        footer={
          <>
            {dirty ? (
              <span className="mr-auto text-sm text-muted-foreground">{copy.unsaved}</span>
            ) : null}
            <Button
              type="submit"
              size="sm"
              className="pointer-coarse:min-h-11"
              disabled={!dirty || needsKey || save.isPending}
            >
              {save.isPending ? copy.saving : copy.save}
            </Button>
          </>
        }
      >
        <SettingRow
          htmlFor={keyId}
          label={c.key.label}
          hintId={`${keyId}-hint`}
          hint={
            <>
              <p>{removeKey ? c.key.removing : c.key.status[settings.key_source]}</p>
              {settings.stored_key_unreadable && !removeKey ? (
                <p className="mt-1 text-destructive">{c.key.unreadable}</p>
              ) : null}
              {typed !== '' && settings.api_key_configured ? (
                <p className="mt-1">{c.key.differentProject}</p>
              ) : null}
            </>
          }
        >
          <div className="flex flex-col gap-2">
            <Input
              id={keyId}
              type="password"
              autoComplete="new-password"
              spellCheck={false}
              aria-describedby={`${keyId}-hint`}
              placeholder={
                settings.api_key_configured ? c.key.placeholderSet : c.key.placeholderUnset
              }
              value={keyDraft}
              disabled={removeKey}
              onChange={(e) => setKeyDraft(e.target.value)}
            />
            {settings.key_source === 'ui' || settings.stored_key_unreadable ? (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="self-start pointer-coarse:min-h-11"
                onClick={() => {
                  setRemoveKey((r) => !r)
                  setKeyDraft('')
                }}
              >
                {removeKey ? c.key.keepUi : c.key.removeUi}
              </Button>
            ) : null}
          </div>
        </SettingRow>
        <SettingRow
          stacked
          label={c.toolkits.label}
          hint={c.toolkits.hint}
          error={needsKey ? c.toolkits.needsKey : undefined}
        >
          <div className="flex flex-col gap-3">
            <div className="flex flex-wrap items-center gap-2">
              <Input
                type="search"
                aria-label={c.toolkits.searchLabel}
                placeholder={c.toolkits.searchPlaceholder}
                autoComplete="off"
                spellCheck={false}
                className="min-w-0 flex-1 @[640px]:max-w-xs"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="pointer-coarse:min-h-11"
                onClick={() => setShown(true)}
              >
                {c.toolkits.selectAll}
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="pointer-coarse:min-h-11"
                onClick={() => setShown(false)}
              >
                {c.toolkits.clear}
              </Button>
              <span className="text-sm text-muted-foreground" aria-live="polite">
                {c.toolkits.count(picked.size, settings.toolkits.length)}
              </span>
            </div>
            {shown.length === 0 ? (
              <p className="text-sm text-muted-foreground">{c.toolkits.none}</p>
            ) : (
              <ul className="grid gap-2 @[640px]:grid-cols-2">
                {shown.map((t) => {
                  const tid = `${id}-tk-${t.toolkit}`
                  return (
                    <li key={t.toolkit}>
                      <Label
                        htmlFor={tid}
                        className="flex cursor-pointer items-start gap-3 rounded-md border border-border p-3 pointer-coarse:min-h-11"
                      >
                        <Checkbox
                          id={tid}
                          checked={picked.has(t.toolkit)}
                          onCheckedChange={(on) => toggle(t.toolkit, on === true)}
                        />
                        <AgentMark name={t.display_name} iconUrl={t.logo_url} size={24} />
                        <span className="min-w-0">
                          <span className="block text-sm font-medium">{t.display_name}</span>
                          {t.description ? (
                            <span className="block text-xs font-normal text-muted-foreground">
                              {t.description}
                            </span>
                          ) : null}
                        </span>
                      </Label>
                    </li>
                  )
                })}
              </ul>
            )}
          </div>
        </SettingRow>
      </SettingRows>
    </form>
  )
}
