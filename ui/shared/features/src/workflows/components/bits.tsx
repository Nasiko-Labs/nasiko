import { Link } from '@tanstack/react-router'
import { Plus } from 'lucide-react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { asButton } from '@nasiko/ui/primitives/button'
import { SectionNav as SharedSectionNav } from '@nasiko/app-core/shell/section-nav'
import { copy } from '../copy'
import type { Tone } from '../logic'

const VARIANT = {
  success: 'success',
  error: 'destructive',
  warning: 'warning',
  info: 'info',
  neutral: 'muted',
} as const satisfies Record<Tone, string>

/** Text plus colour, never a colour alone. */
export function ToneBadge({ tone, children }: { tone: Tone; children: string }) {
  return (
    <Badge variant={VARIANT[tone]} data-tone={tone}>
      {children}
    </Badge>
  )
}

/** A quiet fact chip (steps, runs, tokens, durations). */
export function Chip({ children }: { children: string }) {
  return (
    <Badge variant="outline" className="font-normal text-muted-foreground">
      {children}
    </Badge>
  )
}

const SECTIONS = [
  { to: '/workflows', label: copy.nav.deployed },
  { to: '/workflows/drafts', label: copy.nav.drafts },
  { to: '/workflows/runs', label: copy.nav.runs },
] as const

/** The module's one action, in the top bar on every section. */
export function CreateWorkflowLink() {
  return (
    <Link to="/workflows/new" {...asButton({ size: 'sm', className: 'pointer-coarse:min-h-11' })}>
      <Plus aria-hidden /> {copy.create}
    </Link>
  )
}

/** Deployed · Drafts · Runs: the three lists are one module (the React nav's three items). */
export function SectionNav({ current }: { current: (typeof SECTIONS)[number]['to'] }) {
  return <SharedSectionNav label={copy.nav.label} sections={SECTIONS} current={current} />
}
