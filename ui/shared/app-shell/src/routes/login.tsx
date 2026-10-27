import { zodResolver } from '@hookform/resolvers/zod'
import { useQueryClient } from '@tanstack/react-query'
import { ArrowRight, CircleAlert, Eye, EyeOff } from 'lucide-react'
import { createFileRoute, useLocation, useNavigate } from '@tanstack/react-router'
import { useEffect, useRef, useState } from 'react'
import { useForm } from 'react-hook-form'
import { z } from 'zod/mini'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Button } from '@nasiko/ui/primitives/button'
import { Field, FieldError, FieldGroup, FieldLabel } from '@nasiko/ui/primitives/field'
import { Input } from '@nasiko/ui/primitives/input'
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from '@nasiko/ui/primitives/input-group'
import { Spinner } from '@nasiko/ui/primitives/spinner'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { copy } from '@nasiko/app-core/shell/copy'
import { ForgotPasswordLink } from '@nasiko/app-shell/shell/ForgotPassword'
import { LoginGlow, LoginShowcase } from '@nasiko/app-shell/shell/LoginShowcase'
import { NasikoLockup } from '@nasiko/ui/composites/NasikoMark'
// The waitlist line is off while the free assessment replaces it (user request 2026-10-22).
// import { WaitlistLoginLink } from '@nasiko/app-shell/shell/WaitlistCta'
import { LOGIN_ACCENT, pinAccent } from '@nasiko/app-core/shell/theme'
import {
  endServerSession,
  LOGIN_TIMEOUT_MS,
  resetQueryClient,
  type SignOutResult,
  stopChatTurns,
  stopDeployWork,
  whenSignedOut,
} from '@nasiko/app-shell/shell/signOut'
import { lockedFor, login, type Me, safeRedirect } from '@nasiko/data-access/auth'
import { ApiError, apiFetch, isTimeoutError } from '@nasiko/data-access/client'
import {
  broadcastSession,
  bumpSignInGeneration,
  clearSignedOutMark,
  markExternalSignIn,
  signedOutMark,
  takeExternalSignIn,
  withSessionLock,
} from '@nasiko/app-core/session'
import { env } from '@nasiko/app-core/env'
import { opt } from '@nasiko/ui/lib/search'

export const Route = createFileRoute('/login')({
  validateSearch: z.object({
    redirect: opt(z.string()),
    expired: opt(z.union([z.boolean(), z.string()])),
    // Set by sign out when the logout call failed or timed out (plans/feat-app-shell.md eng D6).
    signout: opt(z.literal('failed')),
    // Set by Settings when a password change came back without a new session (plans/feat-settings.md §1.1).
    password: opt(z.literal('changed')),
    // Back from a `loginMethods` slot's provider (EE single sign-on): its cookie is set, or `#error=<code>` says why not.
    signin: opt(z.literal('external')),
  }),
  component: LoginPage,
})

/** The login form. No LeaveGuard: it navigates on success, and a half-typed password isn't worth guarding. */
const loginSchema = z.object({
  username: z.string().check(z.minLength(1, copy.login.usernameRequired)),
  password: z.string().check(z.minLength(1, copy.login.passwordRequired)),
})
type LoginValues = z.infer<typeof loginSchema>

function LoginPage() {
  const { redirect, expired, signout, password, signin } = Route.useSearch()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const { loginMethods: LoginMethods, passwordSignIn } = useSlots()
  const hash = useLocation({ select: (l) => l.hash })
  const externalError =
    signin === 'external'
      ? (new URLSearchParams(hash.replace(/^#/, '')).get('error') ?? undefined)
      : undefined
  const [finishing, setFinishing] = useState(signin === 'external' && !externalError)
  const finishStarted = useRef(false)
  // An external sign-in comes back here, its cookie already set, to finish as the password one does below.
  useEffect(() => {
    if (!finishing || finishStarted.current) return
    finishStarted.current = true
    const startedHere = takeExternalSignIn()
    void (async () => {
      try {
        // A failed logout's barrier lifts only for a sign-in this tab started (review D1).
        if (signedOutMark() === true && !startedHere) throw new Error('not started in this tab')
        await whenSignedOut()
        await withSessionLock(async () => {
          const me = await apiFetch<Me>('/api/me', { timeout: LOGIN_TIMEOUT_MS })
          bumpSignInGeneration()
          clearSignedOutMark()
          broadcastSession({ type: 'signed-in', sub: me.sub })
        })
        await resetQueryClient(queryClient)
        await navigate({ to: safeRedirect(redirect), replace: true })
      } catch {
        // No session came back (or not ours to resume): the form, as on any visit.
        setFinishing(false)
      }
    })()
  }, [finishing, navigate, queryClient, redirect])
  // Where an external sign-in returns to: here, carrying the page to open after.
  const returnTo = `/login?${new URLSearchParams({ signin: 'external', ...(redirect ? { redirect: safeRedirect(redirect) } : {}) })}`
  const form = useForm<LoginValues>({
    resolver: zodResolver(loginSchema),
    defaultValues:
      env.mode === 'mock'
        ? { username: 'admin', password: 'mock' }
        : { username: '', password: '' },
  })
  const { errors, isSubmitting: busy } = form.formState
  // Carbon while this page is open (arriving by sign-out included); leaving it restores the stored theme.
  useEffect(() => {
    pinAccent(LOGIN_ACCENT)
    return () => pinAccent(null)
  }, [])
  const [showPassword, setShowPassword] = useState(false)
  // Keep Sign in disabled while Try again runs (the session lock already serialises the two calls).
  const [retrying, setRetrying] = useState(false)
  // handleSubmit clears root errors itself at the start of each submit.
  const setError = (message: string) => form.setError('root.server', { message })

  const submit = async ({ username, password }: LoginValues) => {
    if (retrying) return
    // Set only when the login request itself hit our cap: a session lock that never frees throws a
    // TimeoutError too, but that wait is in this browser, not a slow server.
    let loginTimedOut = false
    try {
      // A sign-out still finishing in this tab clears the cache once more at its end: let it finish.
      await whenSignedOut()
      // Before any credential is sent: a turn an expired session left running would otherwise save its
      // reply with the new cookie the moment login sets it (review, security). Such a turn is dead
      // either way, so a failed login loses nothing.
      await stopChatTurns()
      // Same for deploy work: an expired session's uploads, imports and followed builds must not finish (or toast) for
      // whoever signs in next (review: /ship adversarial).
      await stopDeployWork()
      // Under the session lock, so no tab's logout can clear the cookie this sets.
      await withSessionLock(async () => {
        const result = await login(username, password, LOGIN_TIMEOUT_MS).catch((err: unknown) => {
          loginTimedOut = isTimeoutError(err)
          throw err
        })
        // A real sign-in lifts the local sign-out barrier and tells this browser's other tabs who signed
        // in (a tab showing another account reloads; one showing this account keeps its state); the
        // generation tells a sign-out still in flight elsewhere to leave this new session alone.
        bumpSignInGeneration()
        clearSignedOutMark()
        broadcastSession({ type: 'signed-in', sub: result.user_id })
      })
      // A new login may be a different account: drop every cached query so no page shows the
      // previous account's data. The _app route's beforeLoad (ensureQueryData(meQuery)) then
      // loads the new identity.
      await resetQueryClient(queryClient)
      await navigate({ to: safeRedirect(redirect) })
    } catch (err) {
      // Every 401 gets one message, "account disabled" included: the server checks that before the
      // password, so naming it would tell anyone which usernames exist (recommendations §4).
      if (err instanceof ApiError && err.status === 401) setError(copy.login.wrongCredentials)
      // A lock (429 `account_locked`) lasts much longer than the rate limit. `remaining_attempts` on a 401 is never
      // shown: the server only sends it for usernames that exist.
      else if (err instanceof ApiError && err.status === 429) {
        const locked = lockedFor(err.body)
        setError(locked === null ? copy.login.rateLimited : copy.login.locked(locked))
      }
      // Our 10 s cap, not a stopped server: say the server is slow rather than unreachable.
      else if (loginTimedOut) setError(copy.login.timedOut)
      else if (err instanceof ApiError && err.isServerUnreachable) setError(copy.login.unreachable)
      else setError(copy.login.failed)
    }
  }

  return (
    // The prototype's layout (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §1), mirrored and full page:
    // the decorative showcase over the left half from `md` up, the form column on the right. The page keeps Carbon's muted / card background; the form is centred in its column and capped at max-w-md.
    // The prototype's page: two equal columns on its warm backdrop (`--showcase-bg`, per mode), 16 px apart and in.
    <main className="relative isolate grid min-h-svh gap-4 overflow-hidden p-4 [background:var(--showcase-bg)] md:grid-cols-2">
      {/* One glow behind the whole page, so the two halves read as one page. */}
      <LoginGlow />
      {/* aria-hidden and hidden below `md`, so leading the DOM changes neither tab order nor the phone layout. */}
      <LoginShowcase />
      <div className="relative flex flex-col px-4 py-7 md:px-8">
        <div className="flex flex-1 items-center justify-center py-8">
          <div className="flex w-full max-w-115 flex-col gap-3">
            {/* Only while the barrier is set (or unknowable: storage blocked). A stale ?signout=failed after
              a newer sign-in must not offer a logout that would end that new session (review, red team). */}
            {signout === 'failed' && signedOutMark() !== false ? (
              <SignOutFailed
                disabled={busy}
                onBusy={setRetrying}
                onDone={() =>
                  void navigate({ to: '/login', search: { redirect, expired }, replace: true })
                }
              />
            ) : null}
            <NasikoLockup className="h-6 w-auto self-start text-foreground" />
            <h1 className="mt-4 text-[40px] leading-[1.1] font-semibold tracking-[-0.035em]">
              {copy.login.title}
            </h1>
            <p className="mt-1 text-[15.5px] leading-[1.55] text-pretty text-muted-foreground">
              {password
                ? copy.login.passwordChanged
                : expired
                  ? copy.login.expired
                  : copy.login.tagline}
            </p>
            {passwordSignIn ? (
              <form className="mt-3" onSubmit={(e) => void form.handleSubmit(submit)(e)}>
                <FieldGroup className="gap-5.5">
                  <Field data-invalid={!!errors.username} className="gap-2">
                    <FieldLabel htmlFor="login-username" className="text-[13.5px]">
                      {copy.login.username}
                    </FieldLabel>
                    <Input
                      id="login-username"
                      autoComplete="username"
                      placeholder={copy.login.usernamePlaceholder}
                      required
                      className="h-12.5 rounded-[10px] bg-background px-3.5"
                      aria-invalid={!!errors.username}
                      {...form.register('username')}
                    />
                    <FieldError errors={[errors.username]} className="text-xs" />
                  </Field>
                  <Field data-invalid={!!errors.password} className="gap-2">
                    <FieldLabel htmlFor="login-password" className="text-[13.5px]">
                      {copy.login.password}
                    </FieldLabel>
                    <InputGroup className="h-12.5 rounded-[10px] bg-background">
                      <InputGroupInput
                        id="login-password"
                        type={showPassword ? 'text' : 'password'}
                        autoComplete="current-password"
                        placeholder={copy.login.passwordPlaceholder}
                        required
                        aria-invalid={!!errors.password}
                        {...form.register('password')}
                      />
                      <InputGroupAddon align="inline-end">
                        <InputGroupButton
                          size="icon-sm"
                          aria-label={copy.login.showPassword}
                          aria-pressed={showPassword}
                          aria-controls="login-password"
                          onClick={() => setShowPassword((v) => !v)}
                        >
                          {showPassword ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
                        </InputGroupButton>
                      </InputGroupAddon>
                    </InputGroup>
                    <FieldError errors={[errors.password]} className="text-xs" />
                  </Field>
                  <FieldError errors={[errors.root?.server]} className="text-xs" />
                  <Button
                    type="submit"
                    size="lg"
                    className="h-13 rounded-[10px] text-[15px]"
                    disabled={busy || retrying || finishing}
                  >
                    {busy || finishing ? <Spinner aria-hidden /> : null}
                    {busy || finishing ? copy.login.submitting : copy.login.submit}
                    {busy || finishing ? null : <ArrowRight aria-hidden />}
                  </Button>
                </FieldGroup>
              </form>
            ) : null}
            {passwordSignIn ? <ForgotPasswordLink /> : null}
            {LoginMethods ? (
              <LoginMethods
                returnTo={returnTo}
                onStart={markExternalSignIn}
                error={externalError}
              />
            ) : null}
            {/* <WaitlistLoginLink /> */}
          </div>
        </div>
      </div>
    </main>
  )
}

/** The logout call didn't confirm: say so, and let the user repeat only that call. */
function SignOutFailed({
  disabled,
  onBusy,
  onDone,
}: {
  disabled: boolean
  onBusy: (busy: boolean) => void
  onDone: () => void
}) {
  const [state, setState] = useState<'idle' | 'busy' | 'failed'>('idle')
  const retry = async () => {
    setState('busy')
    onBusy(true)
    let result: SignOutResult | 'already' = 'logout-failed'
    try {
      result = await withSessionLock(async () => {
        // Someone signed in since this notice appeared: that session is not the one to end.
        if (signedOutMark() === false) return 'already' as const
        const r = await endServerSession()
        if (r === 'signed-out') {
          clearSignedOutMark()
          broadcastSession({ type: 'signed-out', failed: false })
        }
        return r
      })
    } catch {
      // The lock never came (another tab hung): say it failed, and give the form back.
    } finally {
      onBusy(false)
    }
    if (result === 'logout-failed') setState('failed')
    else onDone()
  }
  return (
    // Polite (`status`), not StateCard's warning `alert`: the notice waits for the user, it doesn't interrupt.
    // The icon colour goes on the Alert: its own `[&>svg]:text-current` outranks a class on the icon.
    <Alert role="status" className="border-warning/50 p-4 [&>svg]:text-warning">
      {/* Warnings keep their icon (§5.3): under Gold the warning orange sits near the brand fill. */}
      <CircleAlert aria-hidden />
      <AlertDescription className="text-foreground">
        <p>{copy.login.signOutFailed}</p>
        {state === 'failed' ? (
          <p className="text-muted-foreground">{copy.login.tryAgainFailed}</p>
        ) : null}
        <Button
          variant="outline"
          size="sm"
          className="mt-2"
          disabled={disabled || state === 'busy'}
          onClick={() => void retry()}
        >
          {state === 'busy' ? copy.login.tryingAgain : copy.login.tryAgain}
        </Button>
      </AlertDescription>
    </Alert>
  )
}
