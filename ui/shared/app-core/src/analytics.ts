/**
 * Reo's sign-in event (docs.reo.dev, Cloud Product step 4: `Reo.identify` after every successful sign-in), so the
 * visits index.html's Reo loader reports are tied to the person who signed in. A no-op where the page has no loader
 * (the dev server, tests, the registry app), where it never loaded (blocked), and for anything but a real address.
 */
interface ReoIdentity {
  username: string
  type: 'email'
  firstname?: string
  lastname?: string
}

declare const Reo: { identify(identity: ReoIdentity): void } | undefined

/** An address, but not a placeholder a server makes up for an identity provider that sent none (`…@<provider>.users`). */
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/
const PLACEHOLDER = /\.users$/i

export function identifySignIn(email: string | null | undefined, displayName?: string | null) {
  if (!email || !EMAIL.test(email) || PLACEHOLDER.test(email)) return
  const [firstname, ...rest] = (displayName ?? '').trim().split(/\s+/)
  const identity: ReoIdentity = {
    username: email,
    type: 'email',
    ...(firstname ? { firstname } : {}),
    ...(rest.length ? { lastname: rest.join(' ') } : {}),
  }
  const send = () => {
    try {
      if (typeof Reo !== 'undefined') Reo.identify(identity)
    } catch {
      // Analytics never breaks a sign-in.
    }
  }
  if (typeof Reo !== 'undefined') return send()
  // Back from an identity provider before reo.js has loaded: once it has, after the loader's own `Reo.init`.
  document
    .querySelector('script[src^="https://static.reo.dev/"]')
    ?.addEventListener('load', send, { once: true })
}
