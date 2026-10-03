/**
 * A module's sub-pages as one row of links under its page header (Workflows: Deployed · Drafts · Runs; Agents: All
 * agents · Your agents · Builds), so a module never needs a second sidebar.
 */
import { Link, type LinkProps } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'

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
  return (
    <nav aria-label={label} className="flex flex-wrap gap-1">
      {sections.map((s) => (
        <Button
          key={s.to}
          asChild
          size="sm"
          variant={s.to === current ? 'secondary' : 'ghost'}
          className={s.to === current ? 'font-medium' : 'font-normal text-muted-foreground'}
        >
          <Link to={s.to} aria-current={s.to === current ? 'page' : undefined}>
            {s.label}
          </Link>
        </Button>
      ))}
    </nav>
  )
}
