/**
 * The config editor (plan §4.3): create, edit or duplicate. Form state is seeded once when the sheet opens and never
 * re-seeded behind the user's back; if the config changes on the server meanwhile, the sheet says so.
 *
 *   open ─► seed form ─► edit ─► Save ─► (key? refetch secrets, re-check the name) ─► POST/PATCH ─► settle
 *                                                                          ├ ok: announce, close (or follow-through)
 *                                                                          └ error: field or footer message
 */
import { zodResolver } from '@hookform/resolvers/zod'
import { ArrowDown, ArrowUp, X } from 'lucide-react'
import { useEffect, useId, useMemo, useState, type ReactNode } from 'react'
import { flushSync } from 'react-dom'
import { Controller, useForm, useWatch, type Control, type UseFormRegister } from 'react-hook-form'
import { z } from 'zod'
import { Disclosure } from '@/components/shared/disclosure'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FieldDescription, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { useAnnounce } from '../announce'
import { useReturnFocus } from '../focus'
import { useSaveConfig, useSecrets, type SaveConfig } from '../api'
import { copy } from '../copy'
import { keySaveFailed, routerError, type ErrorView } from '../errors'
import { fieldErrors } from '../form'
import {
  affects,
  blockedClears,
  catalogIndex,
  countText,
  defaultSecretName,
  fallbackInCatalog,
  isRoutable,
  parseFallback,
  pickableProviders,
  secretNameError,
  type ClearableField,
  type RowRead,
} from '../routing'
import type { CreateConfigBody, LlmConfig, ProviderCatalog, UpdateConfigBody } from '../types'
import { LeaveGuard } from '@/components/shared/leave-guard'
import { LinkButton, Warn } from './bits'

export type EditorMode =
  | { kind: 'create' }
  | { kind: 'edit'; config: LlmConfig }
  | { kind: 'duplicate'; source: LlmConfig; without?: ClearableField; name: string }

/**
 * The field rules. What depends on the server (a field that can't be cleared, an edit with nothing changed, the
 * catalog failing) is checked beside it, in the form.
 */
const schema = z
  .object({
    name: z.string().refine((s) => !!s.trim(), copy.required),
    provider: z.string(),
    model: z.string(),
    tiers: z.tuple([z.string(), z.string(), z.string()]),
    keyMode: z.enum(['saved', 'add', 'platform']),
    savedKey: z.string(),
    newKeyName: z.string(),
    newKeyValue: z.string(),
    fallbacks: z.array(z.string()),
    temperature: z.string(),
    maxTokens: z.string(),
    pinned: z.boolean(),
    pinModel: z.string(),
    useAsDefault: z.boolean(),
  })
  .superRefine((f, ctx) => {
    if (!f.model.trim() && !f.tiers.some(Boolean))
      ctx.addIssue({ code: 'custom', path: ['model'], message: copy.modelHint })
    const secret =
      f.keyMode === 'add'
        ? (secretNameError(f.newKeyName) ?? (!f.newKeyValue ? copy.required : null))
        : f.keyMode === 'saved' && !f.savedKey
          ? copy.required
          : null
    if (secret) ctx.addIssue({ code: 'custom', path: ['secret'], message: secret })
    if (f.pinned && !f.pinModel.trim() && !f.model.trim())
      ctx.addIssue({ code: 'custom', path: ['pinModel'], message: copy.pinModelRequired })
  })

type Form = z.infer<typeof schema>

const TIER_FIELDS = ['tier1_model', 'tier2_model', 'tier3_model'] as const
/** Tier positions, typed so each indexes the tuples above without a check. */
const TIERS = [0, 1, 2] as const
/** Radix Select has no empty value: the saved-key picker's "—". */
const NO_KEY = '__none__'

function seed(
  mode: EditorMode,
  providers: string[],
  hasDefault: boolean,
  secretNames: string[],
): Form {
  const src = mode.kind === 'edit' ? mode.config : mode.kind === 'duplicate' ? mode.source : null
  const drop = mode.kind === 'duplicate' ? mode.without : undefined
  const v = (f: ClearableField) => (drop === f ? null : (src?.[f] ?? null))
  const provider = src?.provider ?? providers[0] ?? 'openai'
  const secret = v('api_key_secret_name') as string | null
  return {
    name: mode.kind === 'duplicate' ? mode.name : (src?.name ?? ''),
    provider,
    model: (v('model') as string | null) ?? '',
    tiers: TIER_FIELDS.map((f) => (v(f) as string | null) ?? '') as [string, string, string],
    keyMode: secret ? 'saved' : mode.kind === 'create' && !secretNames.length ? 'add' : 'platform',
    savedKey: secret ?? '',
    newKeyName: defaultSecretName(provider),
    newKeyValue: '',
    fallbacks: [...(src?.fallback_models ?? [])],
    temperature: v('temperature') === null ? '' : String(v('temperature')),
    maxTokens: v('max_tokens') === null ? '' : String(v('max_tokens')),
    pinned: src?.pinned ?? false,
    pinModel: (v('pinned_model') as string | null) ?? '',
    useAsDefault: mode.kind === 'create' && !hasDefault,
  }
}

interface EditorProps {
  mode: EditorMode
  onClose: () => void
  configs: LlmConfig[] | undefined
  catalog: {
    data?: ProviderCatalog[]
    isError: boolean
    isPending: boolean
    refetch: () => unknown
  }
  customLabels: string[]
  reads: readonly RowRead[]
  onDuplicate: (source: LlmConfig, without?: ClearableField) => void
  onSetDefault: (c: LlmConfig) => void
  onFocusAgents: () => void
}

export function ConfigSheet({
  mode,
  ...props
}: Omit<EditorProps, 'mode'> & { mode: EditorMode | null }) {
  const { onClose } = props
  const returnFocus = useReturnFocus(!!mode)
  return (
    <Sheet
      open={!!mode}
      onOpenChange={(o) => {
        if (!o) onClose()
      }}
    >
      <SheetContent className="w-full gap-0 sm:max-w-sheet-lg" onCloseAutoFocus={returnFocus}>
        {mode ? (
          <Editor
            key={
              mode.kind === 'edit'
                ? mode.config.id
                : mode.kind === 'duplicate'
                  ? `dup:${mode.source.id}:${mode.without ?? ''}`
                  : 'new'
            }
            {...props}
            mode={mode}
          />
        ) : null}
      </SheetContent>
    </Sheet>
  )
}

/** Waits until everything the form needs has settled (plan §4.8: choices wait for their data), then seeds it once. */
function Editor(props: EditorProps) {
  const { mode, configs, catalog } = props
  const secrets = useSecrets()
  // Secrets settle as loaded or failed: a failed list leaves "Add a key" and the platform key (plan §4.8).
  const ready = !!configs && !!catalog.data && (!!secrets.data || secrets.isError)
  const [seeded, setSeeded] = useState(false)
  if (!seeded && (ready || (configs && catalog.isError))) setSeeded(true)
  if (!seeded) {
    return (
      <>
        <Header mode={mode} />
        <div className="p-4 text-sm text-muted-foreground" aria-busy="true">
          {catalog.isError ? copy.catalogFailed : copy.loadingChoices}
        </div>
      </>
    )
  }
  return <EditorForm {...props} />
}

function EditorForm({
  mode,
  onClose,
  configs,
  catalog,
  customLabels,
  reads,
  onDuplicate,
  onSetDefault,
  onFocusAgents,
}: EditorProps) {
  const secrets = useSecrets()
  const save = useSaveConfig()
  const announce = useAnnounce()
  const providers = pickableProviders(customLabels)
  const hasDefault = !!configs?.some((c) => c.is_default)
  const secretNames = (secrets.data ?? []).map((s) => s.name)
  const {
    register,
    control,
    setValue,
    getValues,
    reset: resetForm,
    handleSubmit,
    formState: { isDirty, isSubmitting, touchedFields },
  } = useForm<Form>({
    defaultValues: seed(mode, providers, hasDefault, secretNames),
    resolver: zodResolver(schema),
  })
  const form = useWatch({ control }) as Form
  // The config the edit diffs against: the one opened, or the server's after "Reload".
  const [original, setOriginal] = useState<LlmConfig | null>(
    mode.kind === 'edit' ? mode.config : null,
  )
  const serverNow = mode.kind === 'edit' ? configs?.find((c) => c.id === mode.config.id) : undefined
  const changedElsewhere =
    !!original && !!serverNow && serverNow.updated_at !== original.updated_at && !save.isPending
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<ErrorView | null>(null)
  const [keyMaybeSaved, setKeyMaybeSaved] = useState(false)
  const [ackReplace, setAckReplace] = useState(false)
  const [done, setDone] = useState<null | { config: LlmConfig; affected: number }>(null)
  const [advanced, setAdvanced] = useState(
    () => !!(form.temperature || form.maxTokens || form.pinned),
  )
  const ids = useId()
  const idx = useMemo(() => catalogIndex(catalog.data), [catalog.data])
  // Key hygiene (eng #5): the mutation keeps nothing once the sheet goes.
  const { reset } = save
  useEffect(() => () => reset(), [reset])
  useEffect(() => {
    if (changedElsewhere) announce(copy.changedElsewhere)
  }, [changedElsewhere, announce])

  const setKeyName = (name: string) => {
    if (name !== getValues('newKeyName')) setAckReplace(false)
    setValue('newKeyName', name, { shouldDirty: true })
  }
  const draftClearables: Partial<Record<ClearableField, string | null>> = {
    model: form.model || null,
    temperature: form.temperature || null,
    max_tokens: form.maxTokens || null,
    api_key_secret_name:
      form.keyMode === 'platform'
        ? null
        : form.keyMode === 'saved'
          ? form.savedKey || null
          : form.newKeyName || null,
    pinned_model: form.pinModel || null,
    tier1_model: form.tiers[0] || null,
    tier2_model: form.tiers[1] || null,
    tier3_model: form.tiers[2] || null,
  }
  const blocked = original ? blockedClears(original, draftClearables) : []
  const replacing = form.keyMode === 'add' && secretNames.includes(form.newKeyName)
  const errs = fieldErrors(schema, form)
  // An edit with nothing changed has nothing to send (edits send only changed fields).
  const unchanged =
    mode.kind === 'edit' &&
    !!original &&
    Object.keys(toMutation(mode, form, original).body).length === 0
  const invalid = Object.keys(errs).length > 0 || blocked.length > 0 || catalog.isError || unchanged
  const busy = save.isPending || submitting || isSubmitting
  const count =
    mode.kind === 'edit'
      ? affects('edit', mode.config, reads)
      : form.useAsDefault
        ? affects('set-default', { id: '', is_default: false }, reads)
        : { n: 0, atLeast: false }

  const submit = async (values: Form) => {
    // One save per click: the secrets re-read below runs before the mutation is pending.
    setSubmitting(true)
    setError(null)
    setKeyMaybeSaved(false)
    if (values.keyMode === 'add') {
      // eng #11: the overwrite warning must be about the current list, not the one loaded with the page.
      const fresh = await secrets.refetch()
      const exists = !!fresh.data?.some((s) => s.name === values.newKeyName)
      if (exists && !ackReplace) {
        setAckReplace(true)
        setSubmitting(false)
        return
      }
    }
    const v = toMutation(mode, values, original)
    save.mutate(v, {
      onSuccess: (config) => {
        setValue('newKeyValue', '')
        announce(copy.saved(config.name))
        if (mode.kind === 'duplicate' || (mode.kind === 'create' && count.n === 0)) {
          setDone({
            config,
            affected: mode.kind === 'duplicate' ? affects('edit', mode.source, reads).n : 0,
          })
          return
        }
        onClose()
      },
      onError: (err) => {
        setError(routerError(err))
        if (v.body.secret_value && keySaveFailed(err)) setKeyMaybeSaved(true)
      },
      onSettled: () => {
        save.reset()
        setSubmitting(false)
      },
    })
  }

  if (done) {
    return (
      <>
        <Header mode={mode} />
        <div className="space-y-3 p-4 text-sm">
          <p className="font-medium">{copy.saved(done.config.name)}</p>
          <p className="text-muted-foreground">
            {mode.kind === 'duplicate'
              ? copy.duplicateSaved(String(done.affected))
              : copy.notUsedYet}
          </p>
          <div className="flex flex-wrap gap-2">
            {(mode.kind === 'duplicate' ? mode.source.is_default : true) ? (
              <Button
                size="sm"
                variant="outline"
                className="pointer-coarse:min-h-11"
                onClick={() => {
                  onSetDefault(done.config)
                  onClose()
                }}
              >
                {copy.setDefault}
              </Button>
            ) : null}
            <Button
              size="sm"
              variant="outline"
              className="pointer-coarse:min-h-11"
              onClick={() => {
                onClose()
                onFocusAgents()
              }}
            >
              {mode.kind === 'duplicate' ? copy.changeRouting : copy.attachAction}
            </Button>
            <Button size="sm" className="pointer-coarse:min-h-11" onClick={onClose}>
              {copy.done}
            </Button>
          </div>
        </div>
      </>
    )
  }

  const fieldError = (f: ErrorView['field']) =>
    error && error.field === f ? (
      <p role="alert" className="text-xs text-destructive">
        {error.problem} {error.action}
      </p>
    ) : null
  const blockedNote = (f: ClearableField) =>
    blocked.includes(f) && original ? (
      <Warn>
        {copy.cantClear}{' '}
        <LinkButton
          className="text-xs font-normal text-inherit underline"
          onClick={() => onDuplicate(original, f)}
        >
          {copy.duplicateWithout}
        </LinkButton>
      </Warn>
    ) : null
  const providerOptions = providers.includes(form.provider)
    ? providers
    : [form.provider, ...providers]
  const modelsFor = (p: string) =>
    catalog.data?.find((g) => g.provider === p)?.models.map((m) => m.model) ?? []
  const allModels = (catalog.data ?? []).flatMap((g) =>
    g.models.map((m) => `${g.provider}/${m.model}`),
  )
  const offCatalog = (m: string) =>
    !!m && !!catalog.data && isRoutable(form.provider, customLabels) && !idx.has(form.provider, m)

  return (
    <>
      <Header mode={mode} />
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          if (invalid || busy) e.preventDefault()
          else void handleSubmit(submit)(e)
        }}
        noValidate
      >
        <LeaveGuard when={isDirty && !submitting} />
        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-4 pb-4">
          {changedElsewhere ? (
            <p className="flex items-center gap-2 rounded-md border border-warning/50 bg-warning/5 p-2 text-sm">
              {copy.changedElsewhere}
              <Button
                type="button"
                size="sm"
                variant="outline"
                className="h-7 pointer-coarse:min-h-11"
                onClick={() => {
                  if (!serverNow) return // the notice only shows while serverNow is loaded
                  setOriginal(serverNow)
                  resetForm(
                    seed({ kind: 'edit', config: serverNow }, providers, hasDefault, secretNames),
                  )
                }}
              >
                {copy.reload}
              </Button>
            </p>
          ) : null}
          <FormField id={`${ids}-name`} label={copy.fieldName}>
            <Input
              id={`${ids}-name`}
              {...register('name')}
              maxLength={200}
              aria-invalid={(!!touchedFields.name && !!errs.name) || error?.field === 'name'}
            />
            {fieldError('name')}
          </FormField>
          <FormField id={`${ids}-provider`} label={copy.fieldProvider}>
            <Select
              value={form.provider}
              onValueChange={(p) => {
                if (form.keyMode === 'add' && form.newKeyName === defaultSecretName(form.provider))
                  setKeyName(defaultSecretName(p))
                setValue('provider', p, { shouldDirty: true })
              }}
            >
              <SelectTrigger id={`${ids}-provider`} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {providerOptions.map((p) => (
                  <SelectItem key={p} value={p}>
                    {p}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {!isRoutable(form.provider, customLabels) ? (
              <Warn>
                {form.provider === 'openrouter' ? copy.openrouterHidden : copy.providerGone}
              </Warn>
            ) : null}
            {fieldError('provider')}
          </FormField>
          <FormField id={`${ids}-model`} label={copy.fieldModel} hint={copy.modelHint}>
            <Input
              id={`${ids}-model`}
              list={`${ids}-models`}
              {...register('model')}
              aria-invalid={!!touchedFields.model && !!errs.model}
            />
            <datalist id={`${ids}-models`}>
              {modelsFor(form.provider).map((m) => (
                <option key={m} value={m} />
              ))}
            </datalist>
            {offCatalog(form.model) ? <Warn>{copy.notInCatalog}</Warn> : null}
            {blockedNote('model')}
            {fieldError('model')}
          </FormField>
          <KeyField
            onAddOpen={() => void secrets.refetch()}
            onRetrySecrets={() => void secrets.refetch()}
            form={form}
            control={control}
            register={register}
            setKeyName={setKeyName}
            secretNames={secretNames}
            secretsFailed={secrets.isError}
            ids={ids}
            replacing={replacing || ackReplace}
            secretErr={errs.secret ?? null}
          />
          {blockedNote('api_key_secret_name')}
          {fieldError('secret')}
          {keyMaybeSaved ? <Warn>{copy.keyMaybeSaved}</Warn> : null}

          <FieldSet className="gap-2">
            <FieldLegend variant="label" className="mb-0">
              {copy.tierSection}
            </FieldLegend>
            {TIERS.map((i) => (
              <FormField key={i} id={`${ids}-t${i}`} label={copy.tierLabels[i]}>
                <Input id={`${ids}-t${i}`} list={`${ids}-models`} {...register(`tiers.${i}`)} />
                {offCatalog(form.tiers[i]) ? <Warn>{copy.notInCatalog}</Warn> : null}
                {blockedNote(TIER_FIELDS[i])}
              </FormField>
            ))}
          </FieldSet>

          <Fallbacks
            fallbacks={form.fallbacks}
            onChange={(next) => setValue('fallbacks', next, { shouldDirty: true })}
            form={form}
            allModels={allModels}
            idx={idx}
            catalogLoaded={!!catalog.data}
            ids={ids}
          />

          <Disclosure
            id={`${ids}-advanced`}
            title={copy.advanced}
            open={advanced}
            onToggle={() => setAdvanced(!advanced)}
          >
            <FormField id={`${ids}-temp`} label={copy.temperature}>
              <Input
                id={`${ids}-temp`}
                type="number"
                step="0.1"
                min="0"
                max="2"
                {...register('temperature')}
              />
              {blockedNote('temperature')}
            </FormField>
            <FormField id={`${ids}-max`} label={copy.maxTokens}>
              <Input id={`${ids}-max`} type="number" step="1" min="1" {...register('maxTokens')} />
              {blockedNote('max_tokens')}
            </FormField>
            <CheckField
              control={control}
              name="pinned"
              id={`${ids}-pinned`}
              label={copy.pinModel}
            />
            <p className="text-xs text-muted-foreground">{copy.pinModelHint}</p>
            {form.pinned || form.pinModel ? (
              <FormField id={`${ids}-pin`} label={copy.fieldModel}>
                <Input
                  id={`${ids}-pin`}
                  list={`${ids}-models`}
                  {...register('pinModel')}
                  placeholder={form.model}
                  aria-invalid={!!touchedFields.pinModel && !!errs.pinModel}
                />
                {errs.pinModel ? <p className="text-xs text-destructive">{errs.pinModel}</p> : null}
                {blockedNote('pinned_model')}
                {fieldError('pin')}
              </FormField>
            ) : null}
          </Disclosure>

          {mode.kind === 'create' ? (
            <CheckField
              control={control}
              name="useAsDefault"
              id={`${ids}-default`}
              label={copy.useAsDefault}
            />
          ) : null}
        </div>
        <SheetFooter className="sticky bottom-0 border-t border-border bg-background">
          <p className="text-xs text-muted-foreground">
            {copy.affects(countText(count))} {copy.timing}
          </p>
          {error && !error.field ? (
            <p role="alert" className="text-sm text-destructive">
              {error.problem} {error.cause} {error.action}
            </p>
          ) : null}
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              variant="ghost"
              className="pointer-coarse:min-h-11"
              onClick={onClose}
            >
              {copy.cancel}
            </Button>
            <Button type="submit" className="pointer-coarse:min-h-11" disabled={invalid || busy}>
              {busy ? copy.saving : copy.save}
            </Button>
          </div>
        </SheetFooter>
      </form>
    </>
  )
}

function Header({ mode }: { mode: EditorMode }) {
  return (
    <SheetHeader className="border-b border-border">
      <SheetTitle>
        {mode.kind === 'create'
          ? copy.editorNew
          : mode.kind === 'edit'
            ? copy.editorEdit(mode.config.name)
            : copy.editorDuplicate(mode.source.name)}
      </SheetTitle>
      <SheetDescription>{copy.editorDescription}</SheetDescription>
    </SheetHeader>
  )
}

function FormField({
  id,
  label,
  hint,
  children,
}: {
  id: string
  label: string
  hint?: string
  children: ReactNode
}) {
  return (
    <Field className="gap-1">
      <FieldLabel htmlFor={id}>{label}</FieldLabel>
      {hint ? <FieldDescription className="text-xs">{hint}</FieldDescription> : null}
      {children}
    </Field>
  )
}

function CheckField({
  control,
  name,
  id,
  label,
}: {
  control: Control<Form>
  name: 'pinned' | 'useAsDefault'
  id: string
  label: string
}) {
  return (
    <Controller
      control={control}
      name={name}
      render={({ field }) => (
        <Field orientation="horizontal" className="gap-2">
          <Checkbox
            id={id}
            ref={field.ref}
            checked={field.value}
            onCheckedChange={(c) => field.onChange(c === true)}
            onBlur={field.onBlur}
          />
          <FieldLabel htmlFor={id} className="font-normal">
            {label}
          </FieldLabel>
        </Field>
      )}
    />
  )
}

function KeyField({
  form,
  control,
  register,
  setKeyName,
  secretNames,
  secretsFailed,
  ids,
  replacing,
  secretErr,
  onAddOpen,
  onRetrySecrets,
}: {
  form: Form
  control: Control<Form>
  register: UseFormRegister<Form>
  setKeyName: (name: string) => void
  onRetrySecrets: () => void
  /** eng #11: the replace warning must use the current list, so opening Add a key re-reads it. */
  onAddOpen: () => void
  secretNames: string[]
  secretsFailed: boolean
  ids: string
  replacing: boolean
  secretErr: string | null
}) {
  const noSaved = !secretNames.length
  return (
    <FieldSet className="gap-2">
      <FieldLegend variant="label" className="mb-0">
        {copy.fieldKey}
      </FieldLegend>
      <Controller
        control={control}
        name="keyMode"
        render={({ field }) => (
          <RadioGroup
            aria-label={copy.fieldKey}
            value={field.value}
            onValueChange={(k) => {
              field.onChange(k)
              if (k === 'add') onAddOpen()
            }}
            className="flex flex-wrap gap-3 text-sm"
          >
            {(['saved', 'add', 'platform'] as const).map((k) => (
              <div
                key={k}
                className={cn(
                  'flex items-center gap-1.5',
                  k === 'saved' && noSaved && 'opacity-60',
                )}
              >
                <RadioGroupItem
                  id={`${ids}-key-${k}`}
                  value={k}
                  disabled={k === 'saved' && noSaved}
                />
                <Label htmlFor={`${ids}-key-${k}`} className="font-normal">
                  {k === 'saved' ? copy.keyUseSaved : k === 'add' ? copy.keyAdd : copy.keyPlatform}
                </Label>
              </div>
            ))}
          </RadioGroup>
        )}
      />
      {noSaved && !secretsFailed ? (
        <p className="text-xs text-muted-foreground">{copy.noSavedKeys}</p>
      ) : null}
      {secretsFailed ? (
        <p className="flex flex-wrap items-center gap-2 text-xs text-warning">
          {copy.secretsFailed}
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-7 pointer-coarse:min-h-11"
            onClick={onRetrySecrets}
          >
            {copy.retry}
          </Button>
        </p>
      ) : null}
      {form.keyMode === 'saved' ? (
        <Controller
          control={control}
          name="savedKey"
          render={({ field }) => (
            <Select
              value={field.value || NO_KEY}
              onValueChange={(v) => field.onChange(v === NO_KEY ? '' : v)}
            >
              <SelectTrigger ref={field.ref} aria-label={copy.keyUseSaved} className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NO_KEY}>—</SelectItem>
                {(field.value && !secretNames.includes(field.value)
                  ? [field.value, ...secretNames]
                  : secretNames
                ).map((n) => (
                  <SelectItem key={n} value={n}>
                    {n}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
      ) : null}
      {form.keyMode === 'saved' &&
      form.savedKey &&
      !secretNames.includes(form.savedKey) &&
      !secretsFailed ? (
        <Warn>{copy.missingKey(form.savedKey)}</Warn>
      ) : null}
      {form.keyMode === 'add' ? (
        <div className="space-y-2">
          <Field className="gap-0.5">
            <FieldLabel htmlFor={`${ids}-sname`} className="font-normal">
              {copy.secretName}
            </FieldLabel>
            <Input
              id={`${ids}-sname`}
              value={form.newKeyName}
              onChange={(e) => setKeyName(e.target.value.toUpperCase())}
              autoComplete="off"
              spellCheck={false}
            />
            <FieldDescription className="text-xs">{copy.secretNameHint}</FieldDescription>
          </Field>
          <Field className="gap-0.5">
            <FieldLabel htmlFor={`${ids}-svalue`} className="font-normal">
              {copy.keyValue}
            </FieldLabel>
            <Input
              id={`${ids}-svalue`}
              type="password"
              autoComplete="new-password"
              {...register('newKeyValue')}
              spellCheck={false}
            />
          </Field>
          <div className="space-y-1 rounded-md border border-border bg-muted/50 p-2 text-xs">
            {replacing ? (
              <Warn className="text-xs">{copy.keyReplaces(form.newKeyName)}</Warn>
            ) : null}
            <p className="text-muted-foreground">{copy.keyContainers}</p>
          </div>
          {secretErr && (form.newKeyValue || form.newKeyName) ? (
            <p className="text-xs text-destructive">{secretErr}</p>
          ) : null}
        </div>
      ) : null}
    </FieldSet>
  )
}

function Fallbacks({
  fallbacks,
  onChange,
  form,
  allModels,
  idx,
  catalogLoaded,
  ids,
}: {
  fallbacks: string[]
  onChange: (next: string[]) => void
  form: Form
  allModels: string[]
  idx: ReturnType<typeof catalogIndex>
  catalogLoaded: boolean
  ids: string
}) {
  const [draft, setDraft] = useState('')
  const announce = useAnnounce()
  const addDraft = () => {
    const v = draft.trim()
    if (!v || fallbacks.includes(v)) return
    onChange([...fallbacks, v])
    setDraft('')
  }
  const cfgLike = {
    provider: form.provider,
    api_key_secret_name: form.keyMode === 'platform' ? null : form.savedKey || form.newKeyName,
  }
  const move = (i: number, d: -1 | 1) => {
    const next = [...fallbacks]
    const [e] = next.splice(i, 1)
    if (e === undefined) return // i is always a rendered row's index
    next.splice(i + d, 0, e)
    // Keyboard reordering: commit now, then keep focus on the moved entry's arrow (its other arrow at an end).
    flushSync(() => onChange(next))
    const btn = (dir: string) =>
      document.getElementById(`${ids}-fb${i + d}-${dir}`) as HTMLButtonElement | null
    const own = btn(d < 0 ? 'up' : 'down')
    ;(own && !own.disabled ? own : btn(d < 0 ? 'down' : 'up'))?.focus()
    announce(copy.movedTo(e, i + d + 1))
  }
  return (
    <FieldSet className="gap-2">
      <FieldLegend variant="label" className="mb-0">
        {copy.fallbacksSection}
      </FieldLegend>
      {fallbacks.length ? (
        <ol className="space-y-1">
          {fallbacks.map((f, i) => {
            const p = parseFallback(f, cfgLike)
            return (
              <li
                key={f}
                className="flex items-start gap-2 rounded-md border border-border px-2 py-1.5 text-sm"
              >
                <span className="w-5 shrink-0 text-muted-foreground">{i + 1}.</span>
                <div className="min-w-0 flex-1">
                  <span className="font-mono text-xs">{f}</span>
                  {p.crossProvider ? (
                    <p className="text-xs text-muted-foreground">
                      {copy.platformKeyFallback(p.provider)}
                    </p>
                  ) : null}
                  {catalogLoaded && !fallbackInCatalog(f, cfgLike, idx) ? (
                    <Warn>{copy.notInCatalog}</Warn>
                  ) : null}
                </div>
                <Button
                  id={`${ids}-fb${i}-up`}
                  type="button"
                  size="sm"
                  variant="ghost"
                  className="size-7 px-0 pointer-coarse:size-11"
                  disabled={i === 0}
                  onClick={() => move(i, -1)}
                  aria-label={copy.moveUp(f)}
                >
                  <ArrowUp className="size-3.5" aria-hidden />
                </Button>
                <Button
                  id={`${ids}-fb${i}-down`}
                  type="button"
                  size="sm"
                  variant="ghost"
                  className="size-7 px-0 pointer-coarse:size-11"
                  disabled={i === fallbacks.length - 1}
                  onClick={() => move(i, 1)}
                  aria-label={copy.moveDown(f)}
                >
                  <ArrowDown className="size-3.5" aria-hidden />
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  className="size-7 px-0 pointer-coarse:size-11"
                  onClick={() => onChange(fallbacks.filter((_, j) => j !== i))}
                  aria-label={copy.removeFallback(f)}
                >
                  <X className="size-3.5" aria-hidden />
                </Button>
              </li>
            )
          })}
        </ol>
      ) : null}
      <div className="flex gap-2">
        <Input
          aria-label={copy.addFallback}
          list={`${ids}-all`}
          value={draft}
          placeholder={copy.fallbackPlaceholder}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              addDraft()
            }
          }}
        />
        <datalist id={`${ids}-all`}>
          {allModels.map((m) => (
            <option key={m} value={m} />
          ))}
        </datalist>
        <Button
          type="button"
          variant="outline"
          className="pointer-coarse:min-h-11"
          disabled={!draft.trim() || fallbacks.includes(draft.trim())}
          onClick={addDraft}
        >
          {copy.addFallback}
        </Button>
      </div>
    </FieldSet>
  )
}

/**
 * The request for a save. A create sends everything; an update sends only the fields that differ from `original`,
 * plus the key when one is added. A new key always carries its secret name: the server stores `secret_value` only
 * under the `api_key_secret_name` in the same request (`ensure_secret` returns early without one).
 */
function toMutation(mode: EditorMode, f: Form, original: LlmConfig | null): SaveConfig {
  const num = (s: string) => (s.trim() === '' ? null : Number(s))
  const key =
    f.keyMode === 'saved'
      ? { api_key_secret_name: f.savedKey || null }
      : f.keyMode === 'add'
        ? { api_key_secret_name: f.newKeyName, secret_value: f.newKeyValue }
        : { api_key_secret_name: null }
  const common = {
    name: f.name.trim(),
    provider: f.provider,
    model: f.model.trim() || null,
    fallback_models: f.fallbacks,
    temperature: num(f.temperature),
    max_tokens: num(f.maxTokens),
    pinned: f.pinned,
    pinned_model: f.pinModel.trim() || null,
    tier1_model: f.tiers[0].trim() || null,
    tier2_model: f.tiers[1].trim() || null,
    tier3_model: f.tiers[2].trim() || null,
    ...key,
  }
  if (mode.kind === 'edit' && original) {
    // Only what the user changed: a full body would overwrite fields changed elsewhere since the sheet opened.
    const body: UpdateConfigBody = {}
    for (const [k, v] of Object.entries(common) as [keyof typeof common, unknown][]) {
      if (JSON.stringify(v) !== JSON.stringify(original[k as keyof LlmConfig] ?? null))
        (body as Record<string, unknown>)[k] = v
    }
    if ('secret_value' in key) {
      body.api_key_secret_name = key.api_key_secret_name
      body.secret_value = key.secret_value
    }
    return { mode: 'update', id: original.id, body, wasDefault: original.is_default }
  }
  const body: CreateConfigBody = {
    ...common,
    is_default: mode.kind === 'create' ? f.useAsDefault : false,
  }
  return { mode: 'create', body }
}
