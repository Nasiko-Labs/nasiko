/**
 * The Agent and key step's second half (bring your own key for your own agents; Starter pack agents are free): pick a
 * built-in provider and save its key as
 * the caller's default LLM config, through the router's own `useSaveConfig` (key hygiene as there: the mutation keeps
 * nothing and resets on unmount; the key is never read back). "Where do I get a key?" above the field opens the steps.
 * Then the model, the LLM router's own field (`SuggestInput`: the provider's catalog models as suggestions, any name
 * accepted): never guessed, since a catalog's first model isn't always one the key can call (user 2026-11-06).
 */
import { CircleCheck, CircleHelp, Eye, EyeOff, KeyRound, Lock, TriangleAlert } from 'lucide-react'
import { useEffect, useId, useState } from 'react'
import { PanelError } from '@nasiko/ui/composites/panel'
import { SuggestInput } from '@nasiko/ui/composites/suggest-input'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Button } from '@nasiko/ui/primitives/button'
import { Field, FieldLabel } from '@nasiko/ui/primitives/field'
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from '@nasiko/ui/primitives/input-group'
import {
  Popover,
  PopoverContent,
  PopoverTitle,
  PopoverTrigger,
} from '@nasiko/ui/primitives/popover'
import { RadioGroup, RadioGroupItem } from '@nasiko/ui/primitives/radio-group'
import { Skeleton } from '@nasiko/ui/primitives/skeleton'
import { Spinner } from '@nasiko/ui/primitives/spinner'
import { useCatalog, useSaveConfig } from '@nasiko/features/router/api'
import { defaultSecretName } from '@nasiko/features/router/routing'
import { ApiError } from '@nasiko/data-access/client'
import { useStarterPackOffered } from './api'
import { copy } from './copy'
import { guideProviders, providerName } from './logic'
import { SectionHeading } from './parts'
import { ProviderIcon } from './ProviderIcon'

export function ModelStep({
  connected,
  onConnected,
}: {
  /** What was connected ("Anthropic · claude-…"), shown on return to this step. */
  connected: string | null
  onConnected: (summary: string) => void
}) {
  const ids = useId()
  const catalog = useCatalog()
  const starterPack = useStarterPackOffered()
  const save = useSaveConfig()
  const { reset } = save
  useEffect(() => () => reset(), [reset])
  const providers = guideProviders(catalog.data)
  const [picked, setPicked] = useState<string | null>(null)
  const provider = picked ?? providers[0]?.provider ?? null
  const [key, setKey] = useState('')
  // The eye beside the key, as on the login page's password: to check a paste before Connect.
  const [showKey, setShowKey] = useState(false)
  const [model, setModel] = useState('')
  const [empty, setEmpty] = useState<'key' | 'model' | null>(null)
  const models = providers.find((p) => p.provider === provider)?.models ?? []

  const connect = () => {
    if (!provider) return
    if (!key.trim()) return setEmpty('key')
    if (!model.trim()) return setEmpty('model')
    save.mutate(
      {
        mode: 'create',
        body: {
          name: `${provider}-setup`,
          provider,
          model: model.trim(),
          api_key_secret_name: defaultSecretName(provider),
          secret_value: key.trim(),
          is_default: true,
        },
      },
      {
        onSuccess: () => {
          setKey('')
          onConnected(`${providerName(provider)} · ${model.trim()}`)
        },
      },
    )
  }

  return (
    <section className="flex flex-col gap-4">
      <SectionHeading
        title={copy.model.title}
        intro={starterPack === false ? copy.model.introOwn : copy.model.intro}
      />
      {catalog.isPending ? (
        <Skeleton className="h-24 w-full" />
      ) : catalog.isError ? (
        <PanelError
          error={catalog.error}
          onRetry={() => void catalog.refetch()}
          what={copy.model.loadFailed}
        />
      ) : (
        <>
          {!providers.length ? (
            <p className="text-sm text-muted-foreground">{copy.model.noProviders}</p>
          ) : null}
          <RadioGroup
            aria-label={copy.model.label}
            value={provider ?? ''}
            onValueChange={(v) => {
              setPicked(v)
              // Another provider's model names don't carry over.
              setModel('')
              save.reset()
            }}
            className="grid grid-cols-[repeat(auto-fit,minmax(10rem,1fr))] gap-3"
          >
            {providers.map((p) => (
              <label
                key={p.provider}
                htmlFor={`${ids}-${p.provider}`}
                className="flex cursor-pointer items-center gap-3 rounded-xl border bg-card p-4 hover:bg-accent/50 has-[[data-checked]]:border-foreground/50 has-[[data-checked]]:bg-accent"
              >
                <RadioGroupItem id={`${ids}-${p.provider}`} value={p.provider} />
                <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted">
                  <ProviderIcon provider={p.provider} className="size-4" />
                </span>
                <span className="min-w-0">
                  <span className="block text-sm font-medium">{providerName(p.provider)}</span>
                  <span className="block text-xs text-muted-foreground">
                    {copy.model.models(p.models.length)}
                  </span>
                </span>
              </label>
            ))}
          </RadioGroup>
          {provider ? (
            <Field className="gap-2">
              <div className="flex items-center gap-1">
                <FieldLabel htmlFor={`${ids}-key`}>
                  {copy.model.key(providerName(provider))}
                </FieldLabel>
                <KeyHelp provider={provider} />
              </div>
              <InputGroup className="h-10">
                <InputGroupAddon>
                  <KeyRound aria-hidden />
                </InputGroupAddon>
                <InputGroupInput
                  id={`${ids}-key`}
                  type={showKey ? 'text' : 'password'}
                  placeholder={copy.model.placeholder[provider] ?? copy.model.placeholderOther}
                  autoComplete="off"
                  spellCheck={false}
                  value={key}
                  onChange={(e) => {
                    setKey(e.target.value)
                    setEmpty(null)
                  }}
                />
                <InputGroupAddon align="inline-end">
                  <InputGroupButton
                    size="icon-sm"
                    aria-label={copy.model.showKey}
                    aria-pressed={showKey}
                    aria-controls={`${ids}-key`}
                    onClick={() => setShowKey((v) => !v)}
                  >
                    {showKey ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
                  </InputGroupButton>
                </InputGroupAddon>
              </InputGroup>
            </Field>
          ) : null}
          {provider ? (
            <Field className="gap-2">
              <FieldLabel htmlFor={`${ids}-model`}>{copy.model.model}</FieldLabel>
              <div className="flex gap-2">
                <SuggestInput
                  id={`${ids}-model`}
                  className="h-10 flex-1"
                  suggestions={models}
                  value={model}
                  placeholder={copy.model.modelPlaceholder}
                  autoComplete="off"
                  spellCheck={false}
                  onValueChange={(m) => {
                    setModel(m)
                    setEmpty(null)
                  }}
                />
                <Button
                  type="button"
                  variant="outline"
                  className="h-10"
                  disabled={save.isPending}
                  onClick={connect}
                >
                  {save.isPending ? <Spinner aria-hidden /> : null}
                  {save.isPending ? copy.model.connecting : copy.model.connect}
                </Button>
              </div>
            </Field>
          ) : null}
          {connected ? (
            <Alert role="status">
              <CircleCheck aria-hidden />
              <AlertDescription>{copy.model.connected(connected)}</AlertDescription>
            </Alert>
          ) : null}
          {empty ? (
            <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
              <TriangleAlert aria-hidden className="size-4" />
              {empty === 'key' ? copy.model.keyEmpty : copy.model.modelEmpty}
            </p>
          ) : null}
          {save.isError ? (
            <Alert variant="destructive">
              <TriangleAlert aria-hidden />
              <AlertDescription>
                {copy.model.saveFailed(
                  save.error instanceof ApiError &&
                    typeof save.error.body === 'string' &&
                    save.error.body
                    ? save.error.body
                    : save.error.message,
                )}
              </AlertDescription>
            </Alert>
          ) : null}
          <p className="flex items-start gap-2 text-xs text-muted-foreground">
            <Lock aria-hidden className="mt-px size-3.5 shrink-0" />
            <span>
              {copy.model.vault} {copy.model.manageSecrets}
            </span>
          </p>
        </>
      )}
    </section>
  )
}

/** Where to get a key for the picked provider: its keys page, then copy and paste. */
function KeyHelp({ provider }: { provider: string }) {
  const name = providerName(provider)
  const page = copy.model.keyPage[provider]
  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button type="button" variant="ghost" size="xs" className="ml-auto text-primary-text" />
        }
      >
        <CircleHelp aria-hidden />
        {copy.model.help.label}
      </PopoverTrigger>
      <PopoverContent align="end" className="flex w-80 flex-col gap-3">
        <PopoverTitle className="text-sm">{copy.model.help.title(name)}</PopoverTitle>
        <ol className="flex list-decimal flex-col gap-1.5 pl-4 text-sm text-muted-foreground marker:text-foreground">
          <li>
            {page ? (
              <a
                href={page}
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary-text underline underline-offset-4"
              >
                {copy.model.help.open(name)}
              </a>
            ) : (
              copy.model.help.open(name)
            )}{' '}
            {copy.model.help.signIn}
          </li>
          {copy.model.help.steps.map((s) => (
            <li key={s}>{s}</li>
          ))}
        </ol>
        <p className="border-t pt-3 text-xs text-muted-foreground">{copy.model.help.billing}</p>
      </PopoverContent>
    </Popover>
  )
}
