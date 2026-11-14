/**
 * Reo (visitor and sales-intent analytics), only where a page turns it on (the multi-tenant dashboard on app.nasiko.com)
 * and only after the person agreed. Self-hosted EE, OSS and the desktop apps never call `loadReo`, so `Reo` is never
 * defined there and every function below does nothing.
 *
 * Consent is opt-in everywhere: app.nasiko.com can't tell a visitor's region, so it asks everyone, the way the EU, UK and
 * Switzerland require. A Global Privacy Control or Do Not Track signal is a "no" that the person can't be asked to
 * override from a banner (they can still turn it on in Settings → Privacy).
 */
interface ReoIdentity {
  username: string
  type: 'email'
  firstname?: string
  lastname?: string
}

interface ReoApi {
  init(options: { clientID: string; dnt?: string[] }): void
  identify(identity: ReoIdentity): void
}

declare global {
  interface Window {
    Reo?: ReoApi
  }
  interface Navigator {
    globalPrivacyControl?: boolean
  }
}

/** localStorage: `{ v: 1, marketing, at }`. Per browser, not per account: the choice is about this device's tracking. */
export const CONSENT_KEY = 'nasiko:analytics-consent'
const REO_HOST = 'https://static.reo.dev/'
/** Reo's first-party cookies and storage keys. */
const REO_KEYS = /^__sec__/

type Listener = (marketing: boolean | null) => void
const listeners = new Set<Listener>()

/** The browser says "don't track me" (GPC, or the older DNT). */
export function privacySignal(): boolean {
  if (typeof navigator === 'undefined') return false
  return navigator.globalPrivacyControl === true || navigator.doNotTrack === '1'
}

/** true or false once the person chose; null while they haven't (nothing runs until then). */
export function marketingConsent(): boolean | null {
  try {
    const raw = localStorage.getItem(CONSENT_KEY)
    if (!raw) return null
    const stored = JSON.parse(raw) as { v?: number; marketing?: unknown }
    return stored.v === 1 && typeof stored.marketing === 'boolean' ? stored.marketing : null
  } catch {
    return null
  }
}

/** Reo may run: the person said yes. A yes given in Settings counts even with a GPC/DNT signal on. */
export function reoAllowed(): boolean {
  return marketingConsent() === true
}

/**
 * Saves the choice. Saying no after a yes removes what Reo stored and reloads, so the loaded script stops; the
 * reload is the only reliable way to unload a third-party script.
 */
export function setMarketingConsent(marketing: boolean) {
  const before = marketingConsent()
  try {
    localStorage.setItem(
      CONSENT_KEY,
      JSON.stringify({ v: 1, marketing, at: new Date().toISOString() }),
    )
  } catch {
    // Storage blocked: the choice lasts for this page only, and Reo stays off on the next one.
  }
  listeners.forEach((fn) => fn(marketing))
  if (!marketing && before === true) {
    clearReoData()
    location.reload()
  }
}

export function onConsentChange(fn: Listener): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

function clearReoData() {
  for (const cookie of document.cookie.split(';')) {
    const name = cookie.split('=')[0]?.trim()
    if (!name || !REO_KEYS.test(name)) continue
    const host = location.hostname
    const parent = host.split('.').slice(-2).join('.')
    for (const domain of ['', `; domain=${host}`, `; domain=.${parent}`]) {
      document.cookie = `${name}=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/${domain}`
    }
  }
  try {
    for (const store of [localStorage, sessionStorage]) {
      const keys: string[] = []
      for (let i = 0; i < store.length; i++) {
        const key = store.key(i)
        if (key && REO_KEYS.test(key)) keys.push(key)
      }
      keys.forEach((k) => store.removeItem(k))
    }
  } catch {
    // Storage blocked: nothing of Reo's is in it either.
  }
}

/** Only an id Reo could have issued: it goes into a script URL. */
const CLIENT_ID = /^[a-z0-9]{1,64}$/i

/**
 * Loads reo.js once, if the person agreed. Copy tracking is off (`dnt: ['copy']`): in the app people copy API keys,
 * tokens and code, which must never reach a third party.
 */
export function loadReo(clientId: string) {
  if (!reoAllowed() || !CLIENT_ID.test(clientId)) return
  if (document.querySelector(`script[src^="${REO_HOST}"]`)) return
  const script = document.createElement('script')
  script.src = `${REO_HOST}${clientId}/reo.js`
  script.defer = true
  script.onload = () => {
    try {
      window.Reo?.init({ clientID: clientId, dnt: ['copy'] })
    } catch {
      // Analytics never breaks the app.
    }
  }
  document.head.appendChild(script)
}

/** An address, but not a placeholder a server makes up for an identity provider that sent none (`…@<provider>.users`). */
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/
const PLACEHOLDER = /\.users$/i

/**
 * Reo's sign-in event (docs.reo.dev, Cloud Product step 4): ties this browser's visits to the person who signed in.
 * Only with consent, only where reo.js was loaded, and only for a real address.
 */
export function identifySignIn(email: string | null | undefined, displayName?: string | null) {
  if (!reoAllowed()) return
  if (!email || !EMAIL.test(email) || PLACEHOLDER.test(email)) return
  const [firstname, ...rest] = (displayName ?? '').trim().split(/\s+/)
  const identity: ReoIdentity = {
    username: email.trim().toLowerCase(),
    type: 'email',
    ...(firstname ? { firstname } : {}),
    ...(rest.length ? { lastname: rest.join(' ') } : {}),
  }
  const send = () => {
    try {
      window.Reo?.identify(identity)
    } catch {
      // Analytics never breaks a sign-in.
    }
  }
  if (window.Reo) return send()
  // Signed in before reo.js finished loading: once it has (after `loadReo`'s own `Reo.init`, registered first).
  document
    .querySelector(`script[src^="${REO_HOST}"]`)
    ?.addEventListener('load', send, { once: true })
}

/**
 * Product events (the shared event list, product section; add a name there first). The core names
 * what happened; an edition decides where it goes by registering a sink with `setProductSink`. An
 * edition that registers none sends nothing: every call is then a no-op.
 */
export type ProductEvent =
  | 'app_page_view'
  | 'agent_deploy_started'
  | 'agent_deployed'
  | 'agent_discovered'
  | 'chat_message_sent'
  | 'llm_key_saved'
  | 'error_shown'
/** The only properties a product event carries (the server drops any other): no free text, no names. */
export type ProductProps = Partial<
  Record<
    'route' | 'method' | 'status' | 'provider' | 'mode' | 'area' | 'code' | 'kind' | 'count',
    string | number | boolean
  >
>
type ProductSink = (event: ProductEvent, props: ProductProps) => void
let productSink: ProductSink | null = null

/** An edition's sink for product events. Returns the unregister function. */
export function setProductSink(fn: ProductSink): () => void {
  productSink = fn
  return () => {
    if (productSink === fn) productSink = null
  }
}

/** Records a product event where the edition sends them; never throws, never blocks. */
export function productEvent(event: ProductEvent, props: ProductProps = {}) {
  try {
    productSink?.(event, props)
  } catch {
    // Analytics never breaks the app.
  }
}
