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
import { useForm } from 'react-hook-form'
import { toast } from 'sonner'
import { z } from 'zod'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { LeaveGuard } from '@nasiko/app-core/leave-guard'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PanelError } from '@nasiko/ui/composites/panel'
import { Button } from '@nasiko/ui/primitives/button'
import { Input } from '@nasiko/ui/primitives/input'
import { ApiError } from '@nasiko/data-access/client'
import { useSaveSettings, useSettings } from './api'
import { ComposioSection } from './ComposioSection'
import { SettingRow, SettingRows } from './components/SettingRow'
import { copy } from './copy'
import { registryProblem, valuesOf, type SettingsValues } from './logic'
import { CORE_SECTIONS, type CoreSection, type SettingsSearch } from './search'
import { SETTINGS_FIELDS, type RunningField, type Settings, type SettingsField } from './types'

type Kind = 'text' | 'url'
interface Spec {
  key: SettingsField
  kind: Kind
}

/** Every control maps to a `SettingsUpdate` field (the legacy page's rule: serde drops unknown keys silently). */
const FIELDS: Record<CoreSection, Spec[]> = {
  general: [{ key: 'catalog_tabs', kind: 'text' }],
  limits: [],
  registry: [{ key: 'registry_url', kind: 'url' }],
  // Its own form and Save (ComposioSection), not part of this one.
  mcp: [],
}

/** What the server runs, shown read-only above each section's fields. */
const RUNNING: Record<CoreSection, readonly RunningField[]> = {
  general: ['orchestrator_model', 'router_model', 'default_provider'],
  limits: ['max_flow_depth', 'max_flow_fan_out', 'max_flow_tokens', 'flow_timeout_secs'],
  registry: [],
  mcp: [],
}

const sectionOf = (k: SettingsField): CoreSection =>
  CORE_SECTIONS.find((s) => FIELDS[s].some((f) => f.key === k)) ?? 'general'

// z.object, not z.record: zod is one shared chunk, and an API no other page uses would land in the shell budget.
const fieldsSchema = z.object(
  Object.fromEntries(SETTINGS_FIELDS.map((k) => [k, z.string()])) as Record<
    SettingsField,
    z.ZodString
  >,
)
const schema = fieldsSchema.superRefine((v, ctx) => {
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
      {!layer && section === 'mcp' ? <ComposioSection /> : null}
      <div hidden={!!layer || section === 'mcp'}>
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
    const input = (
      <Input
        {...common}
        type="text"
        inputMode={f.kind === 'url' ? 'url' : undefined}
        autoComplete="off"
        spellCheck={false}
        placeholder={c.placeholder}
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
    <form data-tour="settings-form" onSubmit={(e) => void onSubmit(e)} noValidate>
      <SettingRows
        // Flow limits has nothing to edit: no Save there, unless an edit in another section is waiting for one.
        footer={
          FIELDS[section].length === 0 && !isDirty ? undefined : (
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
          )
        }
      >
        {CORE_SECTIONS.map((s) => (
          // Every section stays mounted, like the legacy page's panels: hidden ones keep their inputs registered.
          <div key={s} hidden={s !== section}>
            {settings.orchestrator_model ? (
              <RunningRows settings={settings} keys={RUNNING[s]} />
            ) : s === 'limits' ? (
              <SettingRow label={copy.sections.limits.label} hint={copy.running.notReported} />
            ) : null}
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

/**
 * What the server runs (its models, default provider and flow limits): set by its environment, so shown, not edited.
 * Rendered only when the server sends `orchestrator_model`: an older one (a multi-tenant workspace not yet upgraded)
 * answers these keys from stored columns nothing reads, which must not be shown as what runs.
 */
function RunningRows({ settings, keys }: { settings: Settings; keys: readonly RunningField[] }) {
  const keyHint = (
    <>
      {copy.models.keyBefore}
      <Link to="/router" className="underline underline-offset-4">
        {copy.models.router}
      </Link>
      {copy.models.keyAfter}
    </>
  )
  return keys.map((k) => {
    const value = settings[k]
    const model = k === 'orchestrator_model' || k === 'router_model'
    return (
      <SettingRow
        key={k}
        label={copy.running[k].label}
        hint={
          model ? (
            <>
              {copy.running[k].hint} {keyHint}
            </>
          ) : (
            copy.running[k].hint
          )
        }
      >
        <p className="font-mono text-sm break-all">
          {value == null || value === ''
            ? copy.running.unknown
            : typeof value === 'number'
              ? value.toLocaleString('en-US')
              : value}
        </p>
        <p className="mt-1 text-xs text-muted-foreground">{copy.running.setByServer}</p>
      </SettingRow>
    )
  })
}
