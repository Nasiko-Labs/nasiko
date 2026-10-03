import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  ACCENT_KEY,
  ACCENTS,
  applyTheme,
  readPrefs,
  resetThemeState,
  setAccent,
  setTheme,
  THEME_KEY,
  THEME_SWITCHING_CLASS,
  THEMES,
  initTheme,
  LOGIN_ACCENT,
  pinAccent,
  pinnedAccentFor,
} from './theme'

/** A controllable prefers-color-scheme. */
function mockSystemDark(initial: boolean) {
  let dark = initial
  const listeners = new Set<() => void>()
  vi.stubGlobal('matchMedia', (q: string) => ({
    get matches() {
      return q.includes('dark') ? dark : false
    },
    media: q,
    addEventListener: (_: string, l: () => void) => listeners.add(l),
    removeEventListener: (_: string, l: () => void) => listeners.delete(l),
  }))
  return (next: boolean) => {
    dark = next
    for (const l of listeners) l()
  }
}

const html = document.documentElement

/** In-memory Storage (Node 24's global localStorage shadows jsdom's and isn't usable here). */
function memoryStorage(): Storage {
  const m = new Map<string, string>()
  return {
    get length() {
      return m.size
    },
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v)),
  }
}

const blocked = (): Storage => {
  const fail = () => {
    throw new Error('blocked')
  }
  return { length: 0, clear: fail, getItem: fail, key: fail, removeItem: fail, setItem: fail }
}

beforeEach(() => {
  vi.stubGlobal('localStorage', memoryStorage())
  resetThemeState()
  html.classList.remove('dark')
  html.removeAttribute('data-theme')
})
afterEach(() => vi.unstubAllGlobals())

describe('theme prefs', () => {
  it('defaults to System and Carbon with empty storage', () => {
    expect(readPrefs()).toEqual({ theme: 'system', accent: 'carbon' })
  })

  it('falls back to the defaults for unknown stored values', () => {
    localStorage.setItem(THEME_KEY, 'sepia')
    localStorage.setItem(ACCENT_KEY, 'magenta')
    expect(readPrefs()).toEqual({ theme: 'system', accent: 'carbon' })
  })

  it('migrates the retired accent presets: indigo keeps its name, the rest read as Carbon', () => {
    for (const [stored, expected] of [
      ['indigo', 'indigo'],
      ['violet', 'carbon'],
      ['slate', 'carbon'],
      ['gold', 'carbon'],
      ['plum', 'plum'],
    ] as const) {
      localStorage.setItem(ACCENT_KEY, stored)
      expect(readPrefs().accent, stored).toBe(expected)
    }
  })

  it('works when storage throws: defaults apply and choices last for the page', () => {
    vi.stubGlobal('localStorage', blocked())
    mockSystemDark(false)
    expect(readPrefs()).toEqual({ theme: 'system', accent: 'carbon' })
    expect(() => setTheme('dark')).not.toThrow()
    expect(html).toHaveClass('dark')
    setAccent('indigo')
    expect(html).toHaveAttribute('data-theme', 'indigo')
  })

  it('applyTheme sets .dark and data-theme on <html>', () => {
    mockSystemDark(false)
    applyTheme({ theme: 'dark', accent: 'plum' })
    expect(html).toHaveClass('dark')
    expect(html).toHaveAttribute('data-theme', 'plum')
    applyTheme({ theme: 'light', accent: 'carbon' })
    expect(html).not.toHaveClass('dark')
  })

  it('stores choices under the openruntime keys', () => {
    mockSystemDark(false)
    setTheme('light')
    setAccent('indigo')
    expect(localStorage.getItem('openruntime.theme')).toBe('light')
    expect(localStorage.getItem('openruntime.accent')).toBe('indigo')
    // Raw strings only (the pre-paint script reads them): no JSON blob under persist's own key.
    expect(localStorage.length).toBe(2)
  })

  it('System follows the OS live; Light and Dark ignore it', () => {
    const setOs = mockSystemDark(false)
    const stop = initTheme()
    setTheme('system')
    expect(html).not.toHaveClass('dark')
    setOs(true)
    expect(html).toHaveClass('dark')
    setTheme('light')
    setOs(true)
    expect(html).not.toHaveClass('dark')
    setTheme('dark')
    setOs(false)
    expect(html).toHaveClass('dark')
    stop()
  })
})

describe('pre-paint script (index.html)', () => {
  const page = readFileSync(join(process.cwd(), 'oss/src/index.html'), 'utf8')

  it('reads the same storage keys as theme.ts', () => {
    expect(page).toContain(`localStorage.getItem('${THEME_KEY}')`)
    expect(page).toContain(`localStorage.getItem('${ACCENT_KEY}')`)
  })

  it('knows the same presets and themes, with the same defaults', () => {
    const list = /\[([^\]]+)\]\.indexOf\(accent\)/.exec(page)![1]!
    expect(list.split(',').map((s) => s.trim().replace(/'/g, ''))).toEqual(ACCENTS.map((a) => a.id))
    expect(page).toContain("accent = 'carbon'")
    expect(page).toContain("theme = 'system'")
    for (const t of THEMES.filter((x) => x.id !== 'system')) expect(page).toContain(`'${t.id}'`)
  })

  it('pins the sign-in screen to the same theme as theme.ts', () => {
    expect(page).toContain(`accent = '${LOGIN_ACCENT}'`)
    expect(page).toContain(String(/\/login\/?$/))
  })

  it('guards storage and uses the favicon mark', () => {
    expect(page).toMatch(/try \{ theme = localStorage/)
    expect(page).toContain('href="/mark-nasiko.svg"')
  })
})

describe('boot and other tabs', () => {
  it('applies the stored choice at boot, even without the pre-paint script', () => {
    mockSystemDark(false)
    localStorage.setItem(THEME_KEY, 'dark')
    localStorage.setItem(ACCENT_KEY, 'plum')
    const stop = initTheme()
    expect(html).toHaveClass('dark')
    expect(html).toHaveAttribute('data-theme', 'plum')
    stop()
  })

  it('follows a choice made in another tab', () => {
    mockSystemDark(false)
    const stop = initTheme()
    expect(html).not.toHaveClass('dark')
    localStorage.setItem(THEME_KEY, 'dark')
    localStorage.setItem(ACCENT_KEY, 'plum')
    window.dispatchEvent(new StorageEvent('storage', { key: THEME_KEY }))
    expect(html).toHaveClass('dark')
    expect(html).toHaveAttribute('data-theme', 'plum')
    expect(readPrefs()).toEqual({ theme: 'dark', accent: 'plum' })
    stop()
  })
})

describe('other tabs, edge cases', () => {
  beforeEach(() => {
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: false,
      media: q,
      addEventListener() {},
      removeEventListener() {},
    }))
    html.className = ''
  })

  it('ignores unrelated storage keys and resets to defaults when another tab clears storage', () => {
    localStorage.setItem(THEME_KEY, 'dark')
    localStorage.setItem(ACCENT_KEY, 'indigo')
    const stop = initTheme()
    expect(html).toHaveClass('dark')
    localStorage.setItem(THEME_KEY, 'light')
    window.dispatchEvent(new StorageEvent('storage', { key: 'something-else' }))
    expect(html).toHaveClass('dark')
    localStorage.clear()
    window.dispatchEvent(new StorageEvent('storage', { key: null }))
    expect(html).not.toHaveClass('dark')
    expect(readPrefs()).toEqual({ theme: 'system', accent: 'carbon' })
    stop()
  })
})

describe('initTheme without matchMedia', () => {
  beforeEach(() => {
    vi.stubGlobal('matchMedia', undefined)
    html.className = ''
  })

  it('treats System as light, follows other tabs, and stops on unsubscribe', () => {
    const stop = initTheme()
    expect(html).not.toHaveClass('dark')
    expect(html.getAttribute('data-theme')).toBe('carbon')
    localStorage.setItem(THEME_KEY, 'dark')
    localStorage.setItem(ACCENT_KEY, 'indigo')
    window.dispatchEvent(new StorageEvent('storage', { key: THEME_KEY }))
    expect(html).toHaveClass('dark')
    expect(html.getAttribute('data-theme')).toBe('indigo')
    stop()
    localStorage.setItem(THEME_KEY, 'light')
    window.dispatchEvent(new StorageEvent('storage', { key: THEME_KEY }))
    expect(html).toHaveClass('dark')
  })
})

describe('switching', () => {
  beforeEach(() => {
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: false,
      media: q,
      addEventListener() {},
      removeEventListener() {},
    }))
    html.className = ''
  })

  it('turns transitions off for the switch, then back on', async () => {
    applyTheme({ theme: 'dark', accent: 'plum' })
    expect(html).toHaveClass(THEME_SWITCHING_CLASS)
    await new Promise((r) => requestAnimationFrame(() => r(null)))
    expect(html).not.toHaveClass(THEME_SWITCHING_CLASS)
    // No change: no flicker of the class.
    applyTheme({ theme: 'dark', accent: 'plum' })
    expect(html).not.toHaveClass(THEME_SWITCHING_CLASS)
  })

  it('index.css disables every transition under the switching class', () => {
    const css = readFileSync(join(process.cwd(), 'common/src/index.css'), 'utf8')
    expect(css).toMatch(
      new RegExp(
        `\\.${THEME_SWITCHING_CLASS},\\s*\\.${THEME_SWITCHING_CLASS} \\*,\\s*\\.${THEME_SWITCHING_CLASS} \\*::before,\\s*\\.${THEME_SWITCHING_CLASS} \\*::after \\{\\s*transition: none !important;`,
      ),
    )
  })
})

describe('the sign-in screen', () => {
  it('matches /login only', () => {
    expect(pinnedAccentFor('/login')).toBe(LOGIN_ACCENT)
    expect(pinnedAccentFor('/login/')).toBe(LOGIN_ACCENT)
    expect(pinnedAccentFor('/agents')).toBeNull()
    expect(pinnedAccentFor('/loginx')).toBeNull()
  })

  it('shows Carbon while pinned and the stored theme again after, mode unchanged', () => {
    mockSystemDark(false)
    localStorage.setItem(THEME_KEY, 'dark')
    localStorage.setItem(ACCENT_KEY, 'plum')
    const stop = initTheme()
    pinAccent(LOGIN_ACCENT)
    expect(html).toHaveAttribute('data-theme', 'carbon')
    expect(html).toHaveClass('dark')
    pinAccent(null)
    expect(html).toHaveAttribute('data-theme', 'plum')
    expect(readPrefs()).toEqual({ theme: 'dark', accent: 'plum' })
    stop()
  })
})
