import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { copy } from './copy'
import {
  EXPANDED_MIN_WIDTH,
  readSidebarCookie,
  SIDEBAR_COOKIE,
  widthDefaultOpen,
} from './sidebarState'

const originalWidth = window.innerWidth
afterEach(() => {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalWidth })
  document.cookie = `${SIDEBAR_COOKIE}=; path=/; max-age=0`
})

describe('sidebar state', () => {
  it('uses the cookie name shadcn writes', () => {
    const src = readFileSync(join(process.cwd(), 'common/src/components/ui/sidebar.tsx'), 'utf8')
    expect(src).toContain(`SIDEBAR_COOKIE_NAME = "${SIDEBAR_COOKIE}"`)
  })

  it('reads only the exact cookie, only true or false', () => {
    expect(readSidebarCookie('a=1; sidebar_state=true; b=2')).toBe(true)
    expect(readSidebarCookie('sidebar_state=false')).toBe(false)
    expect(readSidebarCookie('xsidebar_state=true')).toBeUndefined()
    expect(readSidebarCookie('sidebar_state=maybe')).toBeUndefined()
    expect(readSidebarCookie('')).toBeUndefined()
  })

  it('opens on a first visit from exactly 1280 px (design review 9A)', () => {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: EXPANDED_MIN_WIDTH })
    expect(widthDefaultOpen()).toBe(true)
    Object.defineProperty(window, 'innerWidth', {
      configurable: true,
      value: EXPANDED_MIN_WIDTH - 1,
    })
    expect(widthDefaultOpen()).toBe(false)
    // A stored choice winning over the width is AppShell's rule, tested with the /chat default (v1c test 11).
  })
})

describe('collapse shortcut', () => {
  it('names the key the sidebar listens for', () => {
    const src = readFileSync(join(process.cwd(), 'common/src/components/ui/sidebar.tsx'), 'utf8')
    const key = /SIDEBAR_KEYBOARD_SHORTCUT = "(\w)"/.exec(src)![1]!.toUpperCase()
    expect(copy.collapseShortcut(true)).toBe(`⌘${key}`)
    expect(copy.collapseShortcut(false)).toBe(`Ctrl+${key}`)
    const sidebar = readFileSync(join(process.cwd(), 'common/src/app/shell/AppSidebar.tsx'), 'utf8')
    expect(sidebar).toContain(`'Meta+${key}' : 'Control+${key}'`)
  })
})
