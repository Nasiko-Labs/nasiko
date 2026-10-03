/**
 * Settings → Account → Password: self-service password change (nasiko-cloud-rs `43833316`,
 * ui/common/features/change-password-modal.js, here in the page). `POST /api/auth/change-password` confirms with the
 * current password:
 * - 200: this browser gets a fresh cookie; every other session is revoked.
 * - 204: the password changed, but no new session came back and the cookie was cleared, so sign out and say why.
 * - `{error, code}` errors land on the field the code names (`current_password_incorrect` is a 403, never session
 *   loss); anything else is a toast.
 * The form is remounted after a change (never `reset()`, see CLAUDE.md Settings), so its fields start empty.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Eye, EyeOff } from 'lucide-react'
import { useNavigate } from '@tanstack/react-router'
import { useId, useState } from 'react'
import { useForm } from 'react-hook-form'
import { toast } from 'sonner'
import { signOut } from '@/app/shell/signOut'
import { PageHeader } from '@/components/shared/page-header'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from '@/components/ui/input-group'
import { apiFetch, ApiError } from '@/lib/api/client'
import { meQuery, type Me } from '@/lib/api/auth'
import { PASSWORD_MAX, PASSWORD_MIN, passwordProblem, type PasswordProblem } from '@/lib/password'
import { SettingRow, SettingRows } from './components/SettingRow'
import { copy as settingsCopy } from './copy'

const copy = settingsCopy.password

type Values = { current: string; next: string; confirm: string }

const problemText = (p: PasswordProblem) =>
  p === 'short'
    ? copy.problem.short(PASSWORD_MIN)
    : p === 'long'
      ? copy.problem.long(PASSWORD_MAX)
      : copy.problem[p]

/** The legacy dialog's order: the first failing check is the one reported. */
function firstProblem(v: Values): [keyof Values, string] | null {
  if (!v.current) return ['current', copy.currentRequired]
  if (!v.next) return ['next', copy.nextRequired]
  const p = passwordProblem(v.next)
  if (p) return ['next', problemText(p)]
  if (v.next === v.current) return ['next', copy.same]
  if (v.next !== v.confirm) return ['confirm', copy.mismatch]
  return null
}

/** The server's messages are lowercase fragments; the field reads them as sentences. */
const sentence = (s: string) => s.charAt(0).toUpperCase() + s.slice(1)

export function PasswordPage() {
  const me = useQuery(meQuery)
  const [generation, setGeneration] = useState(0)
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={copy.label} description={copy.sub} />
      {me.data ? (
        <PasswordForm key={generation} me={me.data} onChanged={() => setGeneration((g) => g + 1)} />
      ) : null}
    </div>
  )
}

function PasswordForm({ me, onChanged }: { me: Me; onChanged: () => void }) {
  const id = useId()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [busy, setBusy] = useState(false)
  // Each field reveals on its own (the login page's eye toggle).
  const [shown, setShown] = useState<Record<keyof Values, boolean>>({
    current: false,
    next: false,
    confirm: false,
  })
  const form = useForm<Values>({ defaultValues: { current: '', next: '', confirm: '' } })
  const { errors } = form.formState

  const submit = form.handleSubmit(async (v) => {
    const problem = firstProblem(v)
    if (problem) {
      form.setError(problem[0], { message: problem[1] }, { shouldFocus: true })
      return
    }
    // One request at a time: a second one would send a current password the first already replaced.
    if (busy) return
    setBusy(true)
    try {
      const body = await apiFetch<unknown>('/api/auth/change-password', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ current_password: v.current, new_password: v.next }),
      })
      if (body !== null) {
        toast.success(copy.changed)
        onChanged()
        return
      }
      await signOut({
        queryClient,
        userId: me.sub,
        navigate: (to) =>
          navigate({
            to: '/login',
            search: to.search.signout ? to.search : { password: 'changed' },
          }),
      })
    } catch (err) {
      const b = err instanceof ApiError ? (err.body as { code?: unknown } | null) : null
      const code = typeof b?.code === 'string' ? b.code : ''
      const message = sentence((err instanceof ApiError && err.serverMessage) || copy.failed)
      if (code === 'current_password_incorrect')
        form.setError('current', { message }, { shouldFocus: true })
      else if (code.startsWith('password_'))
        form.setError('next', { message }, { shouldFocus: true })
      else
        toast.error(
          err instanceof ApiError && !err.isServerUnreachable ? message : copy.unreachable,
        )
    } finally {
      setBusy(false)
    }
  })

  const row = (
    name: keyof Values,
    label: string,
    placeholder: string,
    autoComplete: string,
    hint?: string,
  ) => (
    <SettingRow
      htmlFor={`${id}-${name}`}
      label={label}
      hint={hint}
      hintId={hint ? `${id}-${name}-hint` : undefined}
      error={errors[name]?.message}
    >
      <InputGroup>
        <InputGroupInput
          id={`${id}-${name}`}
          type={shown[name] ? 'text' : 'password'}
          autoComplete={autoComplete}
          placeholder={placeholder}
          aria-invalid={!!errors[name]}
          aria-describedby={hint ? `${id}-${name}-hint` : undefined}
          {...form.register(name, {
            // The message always describes the current value: acting on the field clears it.
            onChange: () => form.clearErrors(name),
          })}
        />
        <InputGroupAddon align="inline-end">
          <InputGroupButton
            size="icon-xs"
            aria-label={copy.show(label)}
            aria-pressed={shown[name]}
            aria-controls={`${id}-${name}`}
            onClick={() => setShown((s) => ({ ...s, [name]: !s[name] }))}
          >
            {shown[name] ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
          </InputGroupButton>
        </InputGroupAddon>
      </InputGroup>
    </SettingRow>
  )

  return (
    <form onSubmit={(e) => void submit(e)} noValidate>
      {/* Lets a password manager file the new password under the right account. */}
      <Input type="hidden" autoComplete="username" value={me.username} readOnly />
      <SettingRows
        footer={
          <Button type="submit" size="sm" className="pointer-coarse:min-h-11" disabled={busy}>
            {busy ? copy.submitting : copy.submit}
          </Button>
        }
      >
        {row('current', copy.current, copy.currentPlaceholder, 'current-password')}
        {row(
          'next',
          copy.next,
          copy.nextPlaceholder,
          'new-password',
          copy.policy(PASSWORD_MIN, PASSWORD_MAX),
        )}
        {row('confirm', copy.confirm, copy.confirmPlaceholder, 'new-password')}
      </SettingRows>
    </form>
  )
}
