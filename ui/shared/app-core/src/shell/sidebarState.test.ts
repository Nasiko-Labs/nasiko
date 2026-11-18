import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { copy } from './copy'
import { clearSidebarCookie, readSidebarCookie, SIDEBAR_COOKIE } from './sidebarState'

afterEach(() => {
  document.cookie = `${SIDEBAR_COOKIE}=; path=/; max-age=0`
})

describe('sidebar state', () => {
  it('uses the cookie name shadcn writes', () => {
    const src = readFileSync(join(process.cwd(), 'shared/ui/src/primitives/sidebar.tsx'), 'utf8')
    expect(src).toContain(`SIDEBAR_COOKIE_NAME = "${SIDEBAR_COOKIE}"`)
  })

  it('reads only the exact cookie, only true or false', () => {
    expect(readSidebarCookie('a=1; sidebar_state=true; b=2')).toBe(true)
    expect(readSidebarCookie('sidebar_state=false')).toBe(false)
    expect(readSidebarCookie('xsidebar_state=true')).toBeUndefined()
    expect(readSidebarCookie('sidebar_state=maybe')).toBeUndefined()
    expect(readSidebarCookie('')).toBeUndefined()
  })

  it('sign-out forgets the choice', () => {
    document.cookie = `${SIDEBAR_COOKIE}=true; path=/`
    clearSidebarCookie()
    expect(readSidebarCookie()).toBeUndefined()
  })
})

describe('collapse shortcut', () => {
  it('names the key the sidebar listens for', () => {
    const src = readFileSync(join(process.cwd(), 'shared/ui/src/primitives/sidebar.tsx'), 'utf8')
    const key = /SIDEBAR_KEYBOARD_SHORTCUT = "(\w)"/.exec(src)![1]!.toUpperCase()
    expect(copy.collapseShortcut(true)).toBe(`⌘${key}`)
    expect(copy.collapseShortcut(false)).toBe(`Ctrl+${key}`)
    const sidebar = readFileSync(
      join(process.cwd(), 'shared/app-shell/src/shell/AppSidebar.tsx'),
      'utf8',
    )
    expect(sidebar).toContain(`'Meta+${key}' : 'Control+${key}'`)
  })
})
