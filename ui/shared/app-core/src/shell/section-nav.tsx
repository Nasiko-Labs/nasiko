/**
 * A module's sub-pages as one row of links under its page header (Workflows: Deployed · Drafts · Runs; Agents: All
 * agents · Your agents · Builds), so a module never needs a second sidebar.
 */
import { Link, type LinkProps } from '@tanstack/react-router'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { cn } from '@nasiko/ui/lib/utils'

export interface Section {
  to: NonNullable<LinkProps['to']>
  label: string
}

export function SectionNav({
  label,
  sections,
  current,
}: {
  /** The landmark's name (the module). */
  label: string
  sections: readonly Section[]
  current: Section['to']
}) {
  const at = sections.find((s) => s.to === current)
  return (
    <>
      {/* The module's first section is its nav item ("Agents"); the others are a level below it ("Agents › Builds"). */}
      <TopBar trail={at && at !== sections[0] ? [at.label] : []} />
      {/* Underlined, full width: the module's sections read as the page's tabs, not as loose buttons. The rule is an
          inset shadow, not a border, so the current link's underline covers it from inside the row: anything poking
          out of an overflow-x-auto row scrolls it vertically, a stray scrollbar with "Always show scroll bars" on. */}
      <nav
        data-tour="section-nav"
        aria-label={label}
        className="flex gap-1 overflow-x-auto shadow-[inset_0_-1px_0_var(--color-border)]"
      >
        {sections.map((s) => (
          <Link
            key={s.to}
            to={s.to}
            aria-current={s.to === current ? 'page' : undefined}
            className={cn(
              'inline-flex min-h-10 shrink-0 items-center border-b-2 px-3 text-sm transition-colors focus-visible:outline-2 focus-visible:outline-ring motion-reduce:transition-none',
              s.to === current
                ? 'border-foreground font-medium text-foreground'
                : 'border-transparent text-muted-foreground hover:text-foreground',
            )}
          >
            {s.label}
          </Link>
        ))}
      </nav>
    </>
  )
}
