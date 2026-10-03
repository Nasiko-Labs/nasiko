/**
 * Add or edit a custom provider (plan §4.6, superuser). Test posts the typed base URL and key (the server's test
 * endpoint takes them, not a provider id), so testing a saved provider needs its key typed again. The key is
 * write-only; the base URL must be http(s); test output renders as text only (eng #12). The endpoint type (`kind`) is
 * chosen on a create only: the server can't change it later. Azure also needs its `api-version`.
 */
import { zodResolver } from '@hookform/resolvers/zod'
import { useEffect, useId, useState } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { z } from 'zod'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FieldDescription, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
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
import { safeHttpUrl } from '@/features/agents/normalize'
import { useAnnounce } from '../announce'
import { useReturnFocus } from '../focus'
import { useSaveCustomProvider, useTestCustomProvider } from '../api'
import { copy } from '../copy'
import { routerError } from '../errors'
import { fieldErrors } from '../form'
import type { CustomProvider, ProviderKind, TestCustomProviderResult } from '../types'
import { LeaveGuard } from '@/components/shared/leave-guard'
import { Warn } from './bits'

export type CustomMode = { kind: 'create' } | { kind: 'edit'; provider: CustomProvider }

const KINDS = ['openai', 'azure-openai', 'bedrock-converse'] as const satisfies ProviderKind[]
const URL_PLACEHOLDER: Record<ProviderKind, string> = {
  openai: 'https://llm.example.com/v1',
  'azure-openai': 'https://my-resource.openai.azure.com',
  'bedrock-converse': 'https://bedrock-runtime.us-west-2.amazonaws.com',
}

const origin = (v: string) => safeHttpUrl(v.trim()) && new URL(v.trim()).origin

/**
 * The provider form, for a create or for an edit of `src`. The key is required on a create, and on an edit that moves
 * the base URL to a new host: the server keeps the stored key on a PATCH without one (COALESCE), so the new host would
 * receive it unseen. A default model once set can't be cleared (COALESCE again), nor can an Azure api-version.
 */
const schemaFor = (src: CustomProvider | null) =>
  z
    .object({
      name: z.string().refine((v) => !!v.trim(), copy.required),
      kind: z.enum(KINDS),
      apiVersion: z.string(),
      baseUrl: z
        .string()
        .refine((v) => !!v.trim(), copy.required)
        .refine((v) => !!safeHttpUrl(v.trim()), copy.cpBaseUrlBad),
      key: z.string(),
      defaultModel: z.string().refine((v) => !src?.default_model || !!v.trim(), copy.cantClear),
      sync: z.boolean(),
    })
    .superRefine((f, ctx) => {
      if (!f.key && (!src || hostChanged(src, f.baseUrl)))
        ctx.addIssue({ code: 'custom', path: ['key'], message: copy.required })
      if (f.kind === 'azure-openai' && !f.apiVersion.trim())
        ctx.addIssue({ code: 'custom', path: ['apiVersion'], message: copy.required })
    })
type Form = z.infer<ReturnType<typeof schemaFor>>

const hostChanged = (src: CustomProvider, baseUrl: string) =>
  !!baseUrl.trim() && !!safeHttpUrl(baseUrl.trim()) && origin(baseUrl) !== origin(src.base_url)

const testText = (r: TestCustomProviderResult) =>
  r.chat_error
    ? `${copy.cpTest}: ${r.chat_error}`
    : r.chat_ok
      ? copy.cpTestOk(r.models.length)
      : copy.cpTestModels(r.models.length)

export function CustomProviderSheet({
  mode,
  onClose,
}: {
  mode: CustomMode | null
  onClose: () => void
}) {
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
          <Body
            key={mode.kind === 'edit' ? mode.provider.id : 'new'}
            mode={mode}
            onClose={onClose}
          />
        ) : null}
      </SheetContent>
    </Sheet>
  )
}

function Body({ mode, onClose }: { mode: CustomMode; onClose: () => void }) {
  const ids = useId()
  const announce = useAnnounce()
  const save = useSaveCustomProvider()
  const test = useTestCustomProvider()
  const src = mode.kind === 'edit' ? mode.provider : null
  const [schema] = useState(() => schemaFor(src))
  const {
    control,
    register,
    setValue,
    handleSubmit,
    formState: { isDirty, isSubmitting },
  } = useForm<Form>({
    defaultValues: {
      name: src?.display_name ?? '',
      kind: src?.kind ?? 'openai',
      apiVersion: src?.api_version ?? '',
      baseUrl: src?.base_url ?? '',
      key: '',
      defaultModel: src?.default_model ?? '',
      sync: src?.catalog_sync_enabled ?? true,
    },
    resolver: zodResolver(schema),
  })
  const { name, kind, apiVersion, baseUrl, key, defaultModel, sync } = useWatch({
    control,
  }) as Form
  const [saveError, setSaveError] = useState<unknown>(null)
  const errs = fieldErrors(schema, { name, kind, apiVersion, baseUrl, key, defaultModel, sync })
  const azure = kind === 'azure-openai'
  // Sent only for Azure: the server ignores it elsewhere, and keeps its column null there.
  const version = azure ? { api_version: apiVersion.trim() } : {}
  const urlBad = !!baseUrl && !safeHttpUrl(baseUrl.trim())
  const modelBlocked = !!errs.defaultModel
  const newHost = !!src && hostChanged(src, baseUrl)
  const invalid = Object.keys(errs).length > 0
  const busy = save.isPending || isSubmitting
  const { reset: resetSave } = save
  const { reset: resetTest } = test
  // Key hygiene (eng #5): nothing with the key survives the sheet.
  useEffect(
    () => () => {
      resetSave()
      resetTest()
    },
    [resetSave, resetTest],
  )

  const submit = () => {
    const body =
      mode.kind === 'create'
        ? {
            mode: 'create' as const,
            body: {
              display_name: name.trim(),
              base_url: baseUrl.trim(),
              kind,
              ...version,
              api_key: key,
              default_model: defaultModel.trim() || null,
              catalog_sync_enabled: sync,
            },
          }
        : {
            mode: 'update' as const,
            id: mode.provider.id,
            body: {
              display_name: name.trim(),
              base_url: baseUrl.trim(),
              ...version,
              ...(key ? { api_key: key } : {}),
              ...(defaultModel.trim() ? { default_model: defaultModel.trim() } : {}),
              catalog_sync_enabled: sync,
            },
          }
    setSaveError(null)
    save.mutate(body, {
      onError: (e) => setSaveError(e),
      onSuccess: (out) => {
        setValue('key', '')
        announce(
          out && 'defaultModelSet' in out && !out.defaultModelSet
            ? copy.cpSavedNoModel(name.trim())
            : copy.saved(name.trim()),
        )
        onClose()
      },
      onSettled: () => save.reset(),
    })
  }

  const result = test.data
  return (
    <>
      <SheetHeader className="border-b border-border">
        <SheetTitle>
          {mode.kind === 'create' ? copy.cpNew : copy.cpEdit(mode.provider.display_name)}
        </SheetTitle>
        <SheetDescription>{copy.cpDescription}</SheetDescription>
      </SheetHeader>
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          if (invalid || busy) e.preventDefault()
          else void handleSubmit(submit)(e)
        }}
        noValidate
      >
        <LeaveGuard when={isDirty && !busy} />
        <div className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-sm">
          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-n`}>{copy.cpName}</FieldLabel>
            <Input id={`${ids}-n`} {...register('name')} maxLength={100} />
          </Field>
          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-t`}>{copy.cpKind}</FieldLabel>
            <Select
              value={kind}
              disabled={!!src}
              onValueChange={(v) => setValue('kind', v as ProviderKind, { shouldDirty: true })}
            >
              <SelectTrigger id={`${ids}-t`} className="w-full pointer-coarse:min-h-11">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {KINDS.map((k) => (
                  <SelectItem key={k} value={k}>
                    {copy.cpKinds[k]}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <FieldDescription className="text-xs">
              {src ? copy.cpKindLocked : copy.cpKindHint[kind]}
            </FieldDescription>
          </Field>
          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-u`}>{copy.cpBaseUrl}</FieldLabel>
            <Input
              id={`${ids}-u`}
              {...register('baseUrl')}
              inputMode="url"
              placeholder={URL_PLACEHOLDER[kind]}
              aria-invalid={urlBad}
            />
            {urlBad ? (
              <p role="alert" className="text-xs text-destructive">
                {copy.cpBaseUrlBad}
              </p>
            ) : (
              <FieldDescription className="text-xs">{copy.cpBaseUrlHint[kind]}</FieldDescription>
            )}
          </Field>
          {azure ? (
            <Field className="gap-1">
              <FieldLabel htmlFor={`${ids}-v`}>{copy.cpApiVersion}</FieldLabel>
              <Input
                id={`${ids}-v`}
                {...register('apiVersion')}
                placeholder="2024-10-21"
                spellCheck={false}
              />
              {src?.api_version && !apiVersion.trim() ? <Warn>{copy.cantClear}</Warn> : null}
            </Field>
          ) : null}
          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-k`}>{copy.cpKey}</FieldLabel>
            <Input
              id={`${ids}-k`}
              type="password"
              autoComplete="new-password"
              {...register('key')}
              spellCheck={false}
            />
            {newHost ? (
              <Warn>{copy.cpKeyNewHost}</Warn>
            ) : src?.api_key_set ? (
              <p className="text-xs text-muted-foreground">
                {copy.keySaved}. {copy.cpKeyKeep}
              </p>
            ) : null}
          </Field>
          <Field className="gap-1">
            <FieldLabel htmlFor={`${ids}-m`}>{copy.cpDefaultModel}</FieldLabel>
            <Input id={`${ids}-m`} {...register('defaultModel')} />
            {modelBlocked ? <Warn>{copy.cantClear}</Warn> : null}
          </Field>
          <Controller
            control={control}
            name="sync"
            render={({ field }) => (
              <Field orientation="horizontal" className="gap-2">
                <Checkbox
                  id={`${ids}-s`}
                  ref={field.ref}
                  checked={field.value}
                  onCheckedChange={(c) => field.onChange(c === true)}
                  onBlur={field.onBlur}
                />
                <FieldLabel htmlFor={`${ids}-s`} className="font-normal">
                  {copy.cpSync}
                </FieldLabel>
              </Field>
            )}
          />
          <div className="space-y-1 rounded-md border border-border p-2">
            <Button
              className="pointer-coarse:min-h-11"
              type="button"
              size="sm"
              variant="outline"
              disabled={!key || !baseUrl.trim() || urlBad || !!errs.apiVersion || test.isPending}
              onClick={() =>
                test.mutate(
                  {
                    base_url: baseUrl.trim(),
                    api_key: key,
                    kind,
                    ...version,
                    model: defaultModel.trim() || null,
                  },
                  {
                    onSuccess: (r) => announce(testText(r)),
                    onError: (e) => announce(routerError(e).problem),
                  },
                )
              }
            >
              {test.isPending ? copy.cpTesting : copy.cpTest}
            </Button>
            {!key ? <p className="text-xs text-muted-foreground">{copy.cpTestNeedsKey}</p> : null}
            {result ? (
              <p
                className={
                  result.chat_ok || !defaultModel.trim() ? 'text-xs' : 'text-xs text-destructive'
                }
              >
                {testText(result)}
              </p>
            ) : null}
            {result?.models.length ? (
              <p className="font-mono text-xs text-muted-foreground">{result.models.join(', ')}</p>
            ) : null}
            {test.isError ? (
              <p role="alert" className="text-xs text-destructive">
                {routerError(test.error).problem}
              </p>
            ) : null}
          </div>
        </div>
        <SheetFooter className="sticky bottom-0 border-t border-border bg-background">
          {saveError ? (
            <p role="alert" className="text-sm text-destructive">
              {routerError(saveError).problem} {routerError(saveError).cause}
            </p>
          ) : null}
          <div className="flex justify-end gap-2">
            <Button
              className="pointer-coarse:min-h-11"
              type="button"
              variant="ghost"
              onClick={onClose}
            >
              {copy.cancel}
            </Button>
            <Button className="pointer-coarse:min-h-11" type="submit" disabled={invalid || busy}>
              {busy ? copy.saving : copy.save}
            </Button>
          </div>
        </SheetFooter>
      </form>
    </>
  )
}
