/**
 * The OSS app's ways onto the Nasiko waitlist: the early-access card at the foot of the sidebar (a ticket-icon row in
 * the rail) and the login page's line under Sign in. Shown only in the OSS edition and only when
 * the waitlist URL is valid (`env.waitlistUrl`, tagged `ref=oss-app`); every link opens a new tab and
 * carries nothing about the user.
 */
import { ExternalLink, Ticket } from 'lucide-react'
import { useEdition } from '@/app/edition-context'
import { Button } from '@/components/ui/button'
import {
  SidebarGroup,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from '@/components/ui/sidebar'
import { env } from '@/lib/env'
import { cn } from '@/lib/utils'
import { copy } from './copy'
import { GROUP, ROW } from './rowStyles'

/** The waitlist page, or null where the edition or the build shows none. */
function useWaitlistUrl(): string | null {
  return useEdition() === 'oss' ? env.waitlistUrl : null
}

/** The sidebar's card (expanded sidebar and phone sheet) and its rail row (collapsed). */
export function WaitlistCard() {
  const url = useWaitlistUrl()
  if (!url) return null
  return (
    <SidebarGroup className={cn(GROUP, 'py-2')}>
      {/* Hidden, not unmounted, in the rail, so a collapse never restarts the badge's loop. */}
      <div className="relative mt-2.5 flex flex-col gap-1.5 rounded-lg border bg-card p-3 pt-4.5 group-data-[collapsible=icon]:hidden">
        <EarlyAccessBadge />
        <p className="text-sm leading-snug font-semibold">{copy.waitlist.title}</p>
        <p className="text-xs leading-normal text-muted-foreground">{copy.waitlist.line}</p>
        <Button asChild variant="outline" size="sm" className="mt-1.5 w-full">
          <a href={url} target="_blank" rel="noopener noreferrer">
            {copy.waitlist.cta}
            <ExternalLink aria-hidden className="size-3.5" />
            {/* The space outside the span: an accessible name joins inline children without one. */}{' '}
            <span className="sr-only">{copy.waitlist.newTab}</span>
          </a>
        </Button>
      </div>
      <SidebarMenu className="hidden group-data-[collapsible=icon]:flex">
        <SidebarMenuItem>
          <SidebarMenuButton
            asChild
            tooltip={copy.waitlist.title}
            className={cn(ROW, 'border-sidebar-border bg-card')}
          >
            <a
              href={url}
              target="_blank"
              rel="noopener noreferrer"
              aria-label={`${copy.waitlist.title} ${copy.waitlist.newTab}`}
            >
              <Ticket aria-hidden />
            </a>
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    </SidebarGroup>
  )
}

/**
 * "Early access" on the card's top edge: our take on Aceternity's Moving Border (CSS, `--animate-orbit`), a glint
 * circling the pill's 1 px edge, with the flag's cloth waving (`--animate-flag-wave`). The motion is decorative
 * (aria-hidden, no pointer events); under reduced motion the glint is hidden and the flag still.
 */
function EarlyAccessBadge() {
  return (
    <span className="absolute -top-2.75 left-2.5 flex h-5.5 overflow-hidden rounded-full bg-muted-foreground/50 p-px ring-3 ring-sidebar">
      <span
        aria-hidden
        className="pointer-events-none absolute top-1/2 left-1/2 size-45 -translate-1/2 animate-orbit bg-[conic-gradient(from_0deg,transparent_0deg_260deg,var(--muted-foreground)_310deg,var(--primary-foreground)_340deg,transparent_360deg)] motion-reduce:hidden"
      />
      <span className="relative flex items-center gap-1.25 rounded-full bg-primary pr-2 pl-1.5 font-mono text-3xs font-medium tracking-[0.06em] text-primary-foreground uppercase">
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden
          className="size-3"
        >
          <path d="M4 22V3" />
          <path
            d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z"
            className="origin-left animate-flag-wave [transform-box:fill-box] motion-reduce:animate-none"
          />
        </svg>
        {copy.waitlist.badge}
      </span>
    </span>
  )
}

/** The login page's quiet line under Sign in. */
export function WaitlistLoginLink() {
  const url = useWaitlistUrl()
  if (!url) return null
  return (
    <p className="mt-6 border-t pt-5 text-center text-sm text-muted-foreground">
      {copy.waitlist.loginLead}{' '}
      <a
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        className="font-medium text-foreground underline underline-offset-4 hover:text-muted-foreground"
      >
        {copy.waitlist.cta} <span className="sr-only">{copy.waitlist.newTab}</span>
      </a>
    </p>
  )
}
