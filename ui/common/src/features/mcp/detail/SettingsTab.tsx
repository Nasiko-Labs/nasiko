/**
 * Settings (plans/feat-mcp.md §5, owner or superuser): display name, description, server URL (registered servers; an
 * upload's URL is its container's) and Active. `PATCH` sends only changed fields: the server leaves the rest as they
 * are. The form remounts after a save (`key`), never `reset()` (the React Compiler rule in CLAUDE.md).
 */
import { useId, useState } from 'react'
import { useForm } from 'react-hook-form'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Field, FieldDescription, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { Section } from '@/features/agents/components/bits'
import { useUpdateConnector } from '../api'
import { copy, reason } from '../copy'
import type { ConnectorDetail } from '../types'

type Values = { display_name: string; description: string; url: string; is_active: boolean }

export function SettingsTab({ connector }: { connector: ConnectorDetail }) {
  const [generation, setGeneration] = useState(0)
  return (
    <SettingsForm
      key={`${connector.updated_at ?? ''}-${generation}`}
      connector={connector}
      onSaved={() => setGeneration((g) => g + 1)}
    />
  )
}

function SettingsForm({
  connector: c,
  onSaved,
}: {
  connector: ConnectorDetail
  onSaved: () => void
}) {
  const id = useId()
  const update = useUpdateConnector(c.connector_id)
  const uploaded = c.source_kind === 'uploaded_build'
  const initial: Values = {
    display_name: c.display_name ?? '',
    description: c.description ?? '',
    url: c.url ?? '',
    is_active: c.is_active,
  }
  const form = useForm<Values>({ defaultValues: initial })
  const [active, setActive] = useState(c.is_active)

  const submit = form.handleSubmit((v) => {
    const patch: Record<string, string | boolean> = {}
    for (const k of ['display_name', 'description', 'url'] as const)
      if (v[k].trim() !== initial[k] && !(k === 'url' && uploaded)) patch[k] = v[k].trim()
    if (active !== initial.is_active) patch.is_active = active
    if (!Object.keys(patch).length) {
      toast(copy.nothingChanged)
      return
    }
    update.mutate(patch, {
      onSuccess: () => {
        toast.success(copy.saved)
        onSaved()
      },
    })
  })

  return (
    <Section title={copy.settingsTitle}>
      <form onSubmit={(e) => void submit(e)} noValidate className="max-w-xl space-y-4">
        <FieldGroup className="gap-4">
          <Field className="gap-1.5">
            <FieldLabel htmlFor={`${id}-dn`}>{copy.fields.display_name.label}</FieldLabel>
            <Input id={`${id}-dn`} autoComplete="off" {...form.register('display_name')} />
          </Field>
          <Field className="gap-1.5">
            <FieldLabel htmlFor={`${id}-desc`}>{copy.fields.description.label}</FieldLabel>
            <Textarea id={`${id}-desc`} rows={3} {...form.register('description')} />
          </Field>
          {uploaded ? null : (
            <Field className="gap-1.5">
              <FieldLabel htmlFor={`${id}-url`}>{copy.fields.url.label}</FieldLabel>
              <Input id={`${id}-url`} type="url" autoComplete="off" {...form.register('url')} />
            </Field>
          )}
          <Field orientation="horizontal" className="gap-3">
            <Switch id={`${id}-active`} checked={active} onCheckedChange={setActive} />
            <div>
              <FieldLabel htmlFor={`${id}-active`}>{copy.active}</FieldLabel>
              <FieldDescription>{copy.activeHint}</FieldDescription>
            </div>
          </Field>
        </FieldGroup>
        {update.isError ? (
          <p role="alert" className="text-sm text-destructive">
            {copy.saveFailed(reason(update.error))}
          </p>
        ) : null}
        <Button type="submit" disabled={update.isPending}>
          {update.isPending ? copy.saving : copy.saveChanges}
        </Button>
      </form>
    </Section>
  )
}
