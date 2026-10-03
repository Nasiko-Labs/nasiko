/**
 * Workspace settings (plans/feat-settings.md §1.1), as nasiko-cloud-rs `43833316` ui/common/pages/settings-page.js
 * shows them: General, Flow limits and Registry are sections of ONE form with one Save changes, so an edit in one
 * section survives a switch to another and is saved with the rest. A layer's section (EE: Single sign-on) renders
 * instead, with the form kept mounted (hidden) so its edits survive that switch too.
 * Admin-only (the route redirects everyone else to Secrets).
 */
import { zodResolver } from '@hookform/resolvers/zod'
import { Link } from '@tanstack/react-router'
import { useId, useState } from 'react'
import { Controller, useForm } from 'react-hook-form'
import { toast } from 'sonner'
import { z } from 'zod'
import { useSlots } from '@/app/edition-context'
import { LeaveGuard } from '@/components/shared/leave-guard'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { PanelError } from '@/components/shared/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { ApiError } from '@/lib/api/client'
import { useSaveSettings, useSettings } from './api'
import { SettingRow, SettingRows } from './components/SettingRow'
import { copy } from './copy'
import {
  INT_MAX,
  positiveIntProblem,
  registryProblem,
  valuesOf,
  type SettingsValues,
} from './logic'
import { CORE_SECTIONS, type CoreSection, type SettingsSearch } from './search'
import { SETTINGS_FIELDS, type Settings, type SettingsField } from './types'

type Kind = 'text' | 'number' | 'provider' | 'url'
interface Spec {
  key: SettingsField
  kind: Kind
  /** The column's upper bound (numbers). */
  max?: number
}

/** Every control maps to a `SettingsUpdate` field (the legacy page's rule: serde drops unknown keys silently). */
const FIELDS: Record<CoreSection, Spec[]> = {
  general: [
    { key: 'router_model', kind: 'text' },
    { key: 'default_provider', kind: 'provider' },
    { key: 'catalog_tabs', kind: 'text' },
  ],
  limits: [
    { key: 'max_flow_depth', kind: 'number', max: INT_MAX },
    { key: 'max_flow_fan_out', kind: 'number', max: INT_MAX },
    { key: 'max_flow_tokens', kind: 'number', max: Number.MAX_SAFE_INTEGER },
    { key: 'flow_timeout_secs', kind: 'number', max: INT_MAX },
  ],
  registry: [{ key: 'registry_url', kind: 'url' }],
}

const sectionOf = (k: SettingsField): CoreSection =>
  CORE_SECTIONS.find((s) => FIELDS[s].some((f) => f.key === k)) ?? 'general'

// A blank router model or limit would be stored as NULL, and nothing restores the default (the legacy page sent that).
// z.object, not z.record: zod is one shared chunk, and an API no other page uses would land in the shell budget.
const fieldsSchema = z.object(
  Object.fromEntries(SETTINGS_FIELDS.map((k) => [k, z.string()])) as Record<
    SettingsField,
    z.ZodString
  >,
)
const schema = fieldsSchema.superRefine((v, ctx) => {
  if (!v.router_model?.trim())
    ctx.addIssue({ code: 'custom', path: ['router_model'], message: copy.required })
  for (const f of [...FIELDS.limits])
    if (positiveIntProblem(v[f.key] ?? '', f.max ?? INT_MAX))
      ctx.addIssue({ code: 'custom', path: [f.key], message: copy.positiveInt })
  if (registryProblem(v.registry_url ?? ''))
    ctx.addIssue({
      code: 'custom',
      path: ['registry_url'],
      message: copy.fields.registry_url.invalid,
    })
})

export function SettingsPage({
  search,
  setSearch,
}: {
  search: SettingsSearch
  setSearch: (patch: Partial<SettingsSearch>) => void
}) {
  const { settingsSections } = useSlots()
  const settings = useSettings()
  // A saved form is replaced by a fresh one on the saved row: under the React Compiler a reset() form stops
  // reaching its fields (plans/feat-settings.md §3).
  const [saves, setSaves] = useState(0)
  const layer = settingsSections.find((s) => s.key === search.section)
  const section: CoreSection = (CORE_SECTIONS as readonly string[]).includes(search.section ?? '')
    ? (search.section as CoreSection)
    : 'general'
  const head = copy.sections[section]
  return (
    <div className="flex flex-col gap-6">
      {layer ? <layer.Section /> : <PageHeader title={head.label} description={head.sub} />}
      <div hidden={!!layer}>
        {settings.isPending ? (
          <PageLoader label={copy.loading} />
        ) : settings.isError ? (
          <PanelError
            error={settings.error}
            what={copy.loadWhat}
            onRetry={() => void settings.refetch()}
          />
        ) : (
          <SettingsForm
            key={saves}
            settings={settings.data}
            section={section}
            onSaved={() => setSaves((n) => n + 1)}
            onInvalidIn={(s) => setSearch({ section: s === 'general' ? undefined : s })}
          />
        )}
      </div>
    </div>
  )
}

function SettingsForm({
  settings,
  section,
  onSaved,
  onInvalidIn,
}: {
  settings: Settings
  section: CoreSection
  onSaved: () => void
  /** A field outside the open section failed: show its section. */
  onInvalidIn: (s: CoreSection) => void
}) {
  const id = useId()
  const save = useSaveSettings()
  const form = useForm<SettingsValues>({
    resolver: zodResolver(schema),
    defaultValues: valuesOf(settings),
  })
  const { errors, dirtyFields, isDirty, isSubmitting } = form.formState
  const onSubmit = form.handleSubmit(
    (values) => {
      const edited = new Set(Object.keys(dirtyFields) as SettingsField[])
      save.mutate(
        { values, edited },
        {
          onSuccess: () => {
            toast.success(copy.saved)
            onSaved()
          },
          onError: (err) =>
            toast.error(
              copy.saveFailed(
                (err instanceof ApiError && err.serverMessage) || (err as Error).message,
              ),
            ),
        },
      )
    },
    (errs) => {
      const first = Object.keys(errs)[0] as SettingsField | undefined
      if (first && sectionOf(first) !== section) onInvalidIn(sectionOf(first))
    },
  )
  const control = (f: Spec) => {
    const fid = `${id}-${f.key}`
    const c = copy.fields[f.key]
    const common = {
      id: fid,
      'aria-invalid': !!errors[f.key],
      'aria-describedby': `${fid}-hint`,
    }
    const input =
      f.kind === 'provider' ? (
        <Controller
          control={form.control}
          name={f.key}
          render={({ field }) => (
            <Select value={field.value || 'openai'} onValueChange={field.onChange}>
              <SelectTrigger {...common} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {copy.providers.map(([v, l]) => (
                  <SelectItem key={v} value={v}>
                    {l}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      ) : (
        <Input
          {...common}
          type={f.kind === 'number' ? 'number' : 'text'}
          inputMode={f.kind === 'number' ? 'numeric' : f.kind === 'url' ? 'url' : undefined}
          min={f.kind === 'number' ? 1 : undefined}
          autoComplete="off"
          spellCheck={false}
          placeholder={'placeholder' in c ? c.placeholder : undefined}
          {...form.register(f.key)}
        />
      )
    return (
      <SettingRow
        key={f.key}
        htmlFor={fid}
        label={c.label}
        hint={c.hint}
        hintId={`${fid}-hint`}
        error={errors[f.key]?.message}
      >
        {input}
      </SettingRow>
    )
  }
  return (
    <form onSubmit={(e) => void onSubmit(e)} noValidate>
      <SettingRows
        footer={
          <>
            {isDirty ? (
              <span className="mr-auto text-sm text-muted-foreground">{copy.unsaved}</span>
            ) : null}
            <Button
              type="submit"
              size="sm"
              className="pointer-coarse:min-h-11"
              disabled={save.isPending}
            >
              {save.isPending ? copy.saving : copy.save}
            </Button>
          </>
        }
      >
        {CORE_SECTIONS.map((s) => (
          // Every section stays mounted, like the legacy page's panels: hidden ones keep their inputs registered.
          <div key={s} hidden={s !== section}>
            {FIELDS[s].map(control)}
            {s === 'general' ? (
              <SettingRow
                label={copy.providerKeys.label}
                hint={
                  <>
                    {copy.providerKeys.before}
                    <Link to="/router" className="underline underline-offset-4">
                      {copy.providerKeys.router}
                    </Link>
                    {copy.providerKeys.and}
                    <Link to="/settings/secrets" className="underline underline-offset-4">
                      {copy.providerKeys.secrets}
                    </Link>
                    {copy.providerKeys.after}
                  </>
                }
              />
            ) : null}
            {s === 'registry' ? (
              <SettingRow
                label={copy.registryCredentials.label}
                hint={copy.registryCredentials.body}
              />
            ) : null}
          </div>
        ))}
      </SettingRows>
      <LeaveGuard when={isDirty && !isSubmitting && !save.isPending} samePath />
    </form>
  )
}
