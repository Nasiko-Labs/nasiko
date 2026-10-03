/**
 * The Settings module (plans/feat-settings.md §1), around each of its pages. Its sections are the sidebar's drill-in
 * panel (`SidebarPanel`: one sidebar, never two side by side); with the sidebar collapsed they fall back as Chat's
 * rail does: the same nav in a full-height column beside the rail from 1024 px, a sheet opened from above the page
 * below that. The page scrolls in its own column (the shell fills the viewport on /settings, as on /chat). The rows
 * are nasiko-cloud-rs (`origin/development` a4853db4) `ui/oss/navigation.js` `MODULE_NAVS.settings` plus
 * whatever an edition layer appends:
 * - Workspace: General, (EE: Orchestrator), Flow limits, Registry. `/settings?section=`.
 * - Security: (EE: Single sign-on), Secrets (`/settings/secrets`), Chat context (`/settings/chat-context`; nasiko-cloud-rs
 *   `35c749af`, every user's own).
 * - Account: Appearance (`/settings/appearance`; the lab's, this browser's mode and theme), Password
 *   (`/settings/password`; Change password).
 * A layer's rows come from the `settingsSections` slot, placed after the row they name. A member sees Secrets and
 * Account: the workspace sections are superuser-gated on the API.
 */
import { useQuery } from '@tanstack/react-query'
import { Link, useRouterState } from '@tanstack/react-router'
import { PanelLeft, Settings } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { useSlots } from '@/app/edition-context'
import type { SettingsSection } from '@/app/edition'
import { ACTIVE_ROW, ROW } from '@/app/shell/rowStyles'
import { SidebarPanel } from '@/app/shell/SidebarPanel'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from '@/components/ui/sidebar'
import { meQuery } from '@/lib/api/auth'
import { useMediaQuery } from '@/lib/useMediaQuery'
import { cn } from '@/lib/utils'
import { copy } from './copy'
import { CORE_SECTIONS } from './search'

interface Row {
  key: string
  label: string
  to:
    | '/settings'
    | '/settings/secrets'
    | '/settings/chat-context'
    | '/settings/appearance'
    | '/settings/password'
  section?: string
}

/** `rows` with each layer section inserted after the row it names (or appended). */
function withLayer(rows: Row[], layer: readonly SettingsSection[]): Row[] {
  const out = [...rows]
  for (const s of layer) {
    const row: Row = { key: s.key, label: s.label, to: '/settings', section: s.key }
    const at = s.after ? out.findIndex((r) => r.key === s.after) : -1
    if (at < 0) out.push(row)
    else out.splice(at + 1, 0, row)
  }
  return out
}

export function SettingsLayout({ children }: { children: ReactNode }) {
  const me = useQuery(meQuery)
  // Chat's breakpoint for an inline rail (ChatPage `wide`).
  const wide = useMediaQuery('(min-width: 1024px)')
  const [sheetOpen, setSheetOpen] = useState(false)
  const { settingsSections } = useSlots()
  const location = useRouterState({ select: (s) => s.location })
  const admin = me.data?.is_superuser === true
  const inGroup = (g: 'workspace' | 'security') => settingsSections.filter((s) => s.group === g)
  const groups: { key: string; label: string; rows: Row[] }[] = [
    ...(admin
      ? [
          {
            key: 'workspace',
            label: copy.nav.workspace,
            rows: withLayer(
              CORE_SECTIONS.map((k): Row => ({
                key: k,
                label: copy.sections[k].label,
                to: '/settings',
                // General is the default: its link carries no section.
                section: k === 'general' ? undefined : k,
              })),
              inGroup('workspace'),
            ),
          },
        ]
      : []),
    {
      key: 'security',
      label: copy.nav.security,
      rows: [
        ...withLayer([], admin ? inGroup('security') : []),
        { key: 'secrets', label: copy.secrets.title, to: '/settings/secrets' },
        { key: 'chat-context', label: copy.chatContext.label, to: '/settings/chat-context' },
      ],
    },
    {
      key: 'account',
      label: copy.nav.account,
      rows: [
        { key: 'appearance', label: copy.appearance.label, to: '/settings/appearance' },
        { key: 'password', label: copy.password.label, to: '/settings/password' },
      ],
    },
  ]
  const known = new Set(groups.flatMap((g) => g.rows.map((r) => r.key)))
  const raw = (location.search as { section?: unknown }).section
  // A sub-page (Secrets, Chat context, Appearance, Password) is current by its path; the workspace page by `?section=`.
  const page = groups
    .flatMap((g) => g.rows)
    .find((r) => r.to === location.pathname && r.to !== '/settings')
  const onSubPage = page !== undefined
  const current = page ? page.key : typeof raw === 'string' && known.has(raw) ? raw : 'general'
  const linkProps = (r: Row) => ({
    to: r.to,
    search: r.to === '/settings' ? { section: r.section } : undefined,
    // Sections of the workspace page replace each other; a move between pages is pushed.
    replace: r.to === '/settings' && !onSubPage,
    'aria-current': current === r.key ? ('page' as const) : undefined,
  })
  const nav = (framed = false, onNavigate?: () => void) => (
    <SectionsNav
      groups={groups}
      current={current}
      linkProps={linkProps}
      onNavigate={onNavigate}
      framed={framed}
    />
  )
  return (
    <SidebarPanel panel={nav()}>
      {(inSidebar) => (
        // The page keeps its place in the tree whichever way the sections show, so a collapse never remounts it.
        <div className="flex h-full min-h-0">
          {inSidebar ? null : wide ? (
            <aside className="flex w-60 shrink-0 flex-col border-r border-border">
              {nav(true)}
            </aside>
          ) : (
            <Sheet open={sheetOpen} onOpenChange={setSheetOpen}>
              <SheetContent side="left" className="w-[85vw] max-w-80 p-0">
                <SheetTitle className="sr-only">{copy.nav.label}</SheetTitle>
                {/* The sheet's close button sits top-right; start the list below it. */}
                <div className="flex h-full min-h-0 flex-col pt-4">
                  {nav(true, () => setSheetOpen(false))}
                </div>
              </SheetContent>
            </Sheet>
          )}
          <div className="@container min-h-0 min-w-0 flex-1 overflow-y-auto px-4 py-4">
            <div className="mx-auto flex w-full max-w-3xl flex-col gap-4 @[768px]:pt-6">
              {inSidebar || wide ? null : (
                <Button
                  variant="ghost"
                  size="sm"
                  className="-ml-2 self-start text-muted-foreground pointer-coarse:min-h-11"
                  onClick={() => setSheetOpen(true)}
                >
                  <PanelLeft aria-hidden />
                  {copy.nav.label}
                </Button>
              )}
              <div className="min-w-0">{children}</div>
            </div>
          </div>
        </div>
      )}
    </SidebarPanel>
  )
}

interface NavProps {
  groups: { key: string; label: string; rows: Row[] }[]
  current: string
  linkProps: (r: Row) => {
    to: Row['to']
    search: { section?: string } | undefined
    replace: boolean
    'aria-current': 'page' | undefined
  }
  /** Closes the sheet after a pick. */
  onNavigate?: () => void
  /** Beside the rail or in the sheet: the title is a 48 px header row, as Chat's rail has (in the sidebar it sits under Back). */
  framed?: boolean
}

/**
 * The sections: the app nav's own rows and group headers, the same in the sidebar (the drill-in panel), the column
 * beside the collapsed rail and the phone sheet, as Chat's rail is.
 */
function SectionsNav({ groups, current, linkProps, onNavigate, framed = false }: NavProps) {
  return (
    <nav aria-label={copy.nav.label} className="flex min-h-0 flex-1 flex-col overflow-y-auto pb-2">
      <p
        className={cn(
          'flex items-center gap-2 px-5 text-sm font-semibold',
          framed ? 'h-12 shrink-0' : 'mt-2',
        )}
      >
        <Settings aria-hidden className="size-4 text-muted-foreground" />
        {copy.title}
      </p>
      {groups.map((g) => (
        <SidebarGroup key={g.key} className="px-3 py-0">
          <SidebarGroupLabel className="mt-5 mb-1 h-4 px-2 text-2xs font-medium tracking-[0.04em] text-muted-foreground uppercase">
            {g.label}
          </SidebarGroupLabel>
          <SidebarMenu>
            {g.rows.map((r) => (
              <SidebarMenuItem key={r.key}>
                <SidebarMenuButton
                  asChild
                  isActive={current === r.key}
                  className={cn(ROW, ACTIVE_ROW)}
                >
                  <Link {...linkProps(r)} onClick={onNavigate}>
                    {r.label}
                  </Link>
                </SidebarMenuButton>
              </SidebarMenuItem>
            ))}
          </SidebarMenu>
        </SidebarGroup>
      ))}
    </nav>
  )
}
