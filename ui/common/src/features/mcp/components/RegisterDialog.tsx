/**
 * Register server (plans/feat-mcp.md §4, legacy register modal). Probe detects the auth type (and sets it); each auth
 * type shows only its own fields, and only non-empty ones are sent (`registerBody`). A 201 opens the new server's page,
 * where its credential, OAuth and agent access live. Mounted only while open, so each open starts empty.
 */
import { useNavigate } from '@tanstack/react-router'
import { CheckCircle2, Search, XCircle } from 'lucide-react'
import { useId } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { useProbe, useRegister } from '../api'
import { AUTH_LABEL, AUTH_OPTION, copy, reason } from '../copy'
import { authFields, registerBody, type RegisterValues } from '../logic'
import { AUTH_TYPES } from '../types'

const EMPTY: RegisterValues = {
  name: '',
  display_name: '',
  url: '',
  auth_type: 'none',
  credential_header_name: '',
  basic_username: '',
  basic_password: '',
  oauth_client_id: '',
  oauth_client_secret: '',
  url_param_name: '',
  description: '',
}

const SECRET = new Set<keyof RegisterValues>(['basic_password', 'oauth_client_secret'])
const isHttpUrl = (v: string) => {
  try {
    return ['http:', 'https:'].includes(new URL(v.trim()).protocol)
  } catch {
    return false
  }
}

export function RegisterDialog({ onClose }: { onClose: () => void }) {
  const id = useId()
  const navigate = useNavigate()
  const register = useRegister()
  const probe = useProbe()
  const form = useForm<RegisterValues>({ defaultValues: EMPTY })
  const { errors } = form.formState
  // useWatch, not form.watch: the React Compiler memoises a watch() read.
  const authType = useWatch({ control: form.control, name: 'auth_type' })

  const submit = form.handleSubmit((v) => {
    register.mutate(registerBody(v), {
      onSuccess: (c) => {
        onClose()
        toast.success(copy.registered(c.display_name || c.name))
        void navigate({ to: '/mcp/$connectorId', params: { connectorId: c.connector_id } })
      },
    })
  })

  const runProbe = () => {
    const url = form.getValues('url').trim()
    if (!isHttpUrl(url)) {
      form.setError('url', { message: copy.badUrl }, { shouldFocus: true })
      return
    }
    probe.mutate(url, { onSuccess: (p) => form.setValue('auth_type', p.auth_type) })
  }

  const text = (
    name: Exclude<keyof RegisterValues, 'auth_type' | 'description'>,
    opts: { required?: boolean; url?: boolean } = {},
  ) => {
    const f: { label: string; placeholder?: string; hint?: string } = copy.fields[name]
    return (
      <Field data-invalid={!!errors[name]} className="gap-1.5">
        <FieldLabel htmlFor={`${id}-${name}`}>{f.label}</FieldLabel>
        <Input
          id={`${id}-${name}`}
          type={SECRET.has(name) ? 'password' : opts.url ? 'url' : 'text'}
          autoComplete="off"
          placeholder={f.placeholder}
          aria-invalid={!!errors[name]}
          {...form.register(name, {
            validate: (v) =>
              opts.required && !v.trim()
                ? copy.required
                : opts.url && v.trim() && !isHttpUrl(v)
                  ? copy.badUrl
                  : true,
            onChange: () => form.clearErrors(name),
          })}
        />
        {errors[name] ? (
          <FieldError errors={[errors[name]]} />
        ) : f.hint ? (
          <FieldDescription>{f.hint}</FieldDescription>
        ) : null}
      </Field>
    )
  }

  return (
    <Dialog open onOpenChange={(o) => (o || register.isPending ? null : onClose())}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-lg">
        <form onSubmit={(e) => void submit(e)} noValidate className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>{copy.registerTitle}</DialogTitle>
            <DialogDescription>{copy.registerDesc}</DialogDescription>
          </DialogHeader>
          <FieldGroup className="gap-4">
            {text('name', { required: true })}
            {text('display_name')}
            <div className="space-y-2">
              <div className="flex items-end gap-2">
                <div className="min-w-0 flex-1">{text('url', { required: true, url: true })}</div>
                <Button
                  type="button"
                  variant="outline"
                  disabled={probe.isPending}
                  onClick={runProbe}
                  className={errors.url ? 'mb-6' : undefined}
                >
                  <Search aria-hidden /> {probe.isPending ? copy.probing : copy.probe}
                </Button>
              </div>
              {probe.isSuccess ? (
                <p className="flex items-start gap-1.5 text-sm" role="status">
                  <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-success" aria-hidden />
                  <span>
                    {copy.probeResult(AUTH_LABEL[probe.data.auth_type] ?? probe.data.auth_type)}
                    {probe.data.hint ? (
                      <span className="text-muted-foreground"> — {probe.data.hint}</span>
                    ) : null}
                  </span>
                </p>
              ) : probe.isError ? (
                <p className="flex items-start gap-1.5 text-sm text-destructive" role="alert">
                  <XCircle className="mt-0.5 size-4 shrink-0" aria-hidden />
                  {copy.probeFailed(reason(probe.error))}
                </p>
              ) : null}
            </div>
            <Field className="gap-1.5">
              <FieldLabel htmlFor={`${id}-auth`}>{copy.fields.auth_type.label}</FieldLabel>
              <Controller
                control={form.control}
                name="auth_type"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id={`${id}-auth`} className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {AUTH_TYPES.map((t) => (
                        <SelectItem key={t} value={t}>
                          {AUTH_OPTION[t]}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
            </Field>
            {authFields(authType).map((name) => (
              <div key={name}>
                {/* url_param needs its name (`register_connector`: 400 without it). */}
                {text(name as Exclude<keyof RegisterValues, 'auth_type' | 'description'>, {
                  required: name === 'url_param_name',
                })}
              </div>
            ))}
            <Field className="gap-1.5">
              <FieldLabel htmlFor={`${id}-description`}>{copy.fields.description.label}</FieldLabel>
              <Textarea
                id={`${id}-description`}
                rows={2}
                placeholder={copy.fields.description.placeholder}
                {...form.register('description')}
              />
            </Field>
          </FieldGroup>
          {register.isError ? (
            <p role="alert" className="text-sm text-destructive">
              {copy.registerFailed(reason(register.error))}
            </p>
          ) : null}
          <DialogFooter>
            <Button type="button" variant="outline" disabled={register.isPending} onClick={onClose}>
              {copy.cancel}
            </Button>
            <Button type="submit" disabled={register.isPending}>
              {register.isPending ? copy.registering : copy.registerTitle}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
