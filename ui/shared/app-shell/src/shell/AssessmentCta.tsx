/**
 * The way to the free TokenOps assessment (`/assessment`): a card above the sidebar footer (expanded sidebar and phone
 * sheet) and a clipboard row in the rail. It took the waitlist card's place (WaitlistCta.tsx, now unused) and keeps its
 * shape, but links inside the app and its badge holds still.
 */
import { Link } from '@tanstack/react-router'
import { ClipboardCheck } from 'lucide-react'
import { asButton } from '@nasiko/ui/primitives/button'
import {
  SidebarGroup,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from '@nasiko/ui/primitives/sidebar'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '@nasiko/app-core/shell/copy'
import { GROUP, ROW } from '@nasiko/app-core/shell/rowStyles'

export function AssessmentCard() {
  const { setOpenMobile } = useSidebar()
  const close = () => setOpenMobile(false)
  return (
    <SidebarGroup className={cn(GROUP, 'py-2')}>
      <div className="relative mt-2.5 flex flex-col gap-1.5 rounded-lg border bg-card p-3 pt-4.5 group-data-[collapsible=icon]:hidden">
        <span className="absolute -top-2.75 left-2.5 flex h-5.5 items-center gap-1.25 rounded-full bg-primary pr-2 pl-1.5 font-mono text-3xs font-medium tracking-[0.06em] text-primary-foreground uppercase ring-3 ring-sidebar">
          <ClipboardCheck aria-hidden className="size-3" />
          {copy.assessment.badge}
        </span>
        <p className="text-sm leading-snug font-semibold">{copy.assessment.title}</p>
        <p className="text-xs leading-normal text-muted-foreground">{copy.assessment.line}</p>
        <Link
          to="/assessment"
          onClick={close}
          {...asButton({ variant: 'outline', size: 'sm', className: 'mt-1.5 w-full' })}
        >
          {copy.assessment.cta}
        </Link>
      </div>
      <SidebarMenu className="hidden group-data-[collapsible=icon]:flex">
        <SidebarMenuItem>
          <SidebarMenuButton
            tooltip={copy.assessment.title}
            className={cn(ROW, 'border-sidebar-border bg-card')}
            render={<Link to="/assessment" aria-label={copy.assessment.title} onClick={close} />}
          >
            <ClipboardCheck aria-hidden />
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    </SidebarGroup>
  )
}
