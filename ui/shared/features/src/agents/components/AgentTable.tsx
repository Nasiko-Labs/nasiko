/**
 * The catalog's List view: one table row per agent with the card's facts (AgentCard). A row is one
 * link, like a card: the name links to the agent and stretches over the row, so a click anywhere opens
 * it and middle-click or open-in-new-tab work. Below 640 px the description and owner fold under the
 * name, as on Your agents.
 */
import { Link } from '@tanstack/react-router'
import { memo, type ComponentProps, type ReactNode } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@nasiko/ui/primitives/table'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import { displayStatus, isHarness } from '../status'
import { ROW_SKILLS } from '../tuning'
import type { AgentCard } from './AgentCard'
import { AgentMark, StatusBadge } from './bits'

/** Columns shown from 640 px; below it the name cell carries them. Status, skills (from 1280 px) and owner
 * (`w-px`) shrink to their content, so the description gets the room the name leaves. */
const WIDE = 'hidden sm:table-cell'
const SKILLS = 'hidden xl:table-cell'
/** Until 1440 px the skills column holds one chip (its count says the rest); from there, `ROW_SKILLS`. */
const ALL_CHIPS = 'hidden min-[1440px]:inline-flex'
const ONE_CHIP = 'min-[1440px]:hidden'

export function AgentTable({ children }: { children: ReactNode }) {
  return (
    <div data-tour="agents-grid" className="rounded-lg border border-border">
      <Table aria-label={copy.sections.all}>
        <TableHeader>
          <TableRow className="text-xs hover:bg-transparent">
            <TableHead scope="col" className="px-4 text-muted-foreground">
              {copy.colName}
            </TableHead>
            <TableHead scope="col" className="w-px text-muted-foreground">
              {copy.colStatus}
            </TableHead>
            <TableHead scope="col" className={cn(WIDE, 'text-muted-foreground')}>
              {copy.colDescription}
            </TableHead>
            <TableHead scope="col" className={cn(SKILLS, 'w-px text-muted-foreground')}>
              {copy.colSkills}
            </TableHead>
            <TableHead scope="col" className={cn(WIDE, 'w-px pr-4 text-muted-foreground')}>
              {copy.owner}
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>{children}</TableBody>
      </Table>
    </div>
  )
}

/** Memoized for the same reason as the card: every search keystroke re-renders the page. */
export const AgentTableRow = memo(function AgentTableRow({
  agent,
  meId,
  ownerName,
  reason,
  linkState,
}: ComponentProps<typeof AgentCard>) {
  const harness = isHarness(agent)
  const name = agent.display_name || agent.name
  const mine = agent.owner_id === meId
  const skills = agent.skills ?? []
  const more = (shown: number, className: string) =>
    skills.length > shown ? (
      <Badge variant="outline" className={cn('font-normal', className)}>
        +{skills.length - shown}
      </Badge>
    ) : null
  const description = agent.description || copy.noDescription
  const access = harness ? copy.harnessChip : mine ? copy.yours : copy.availableToYou
  const owner = mine ? undefined : ownerName
  return (
    // `relative`: the containing block of the name's stretched link.
    <TableRow className="relative">
      <TableCell className="max-w-0 min-w-40 px-4 py-1.5 whitespace-normal sm:w-1/4">
        {/* One line per agent, so a screen holds more rows than cards: the slug trails the name. */}
        <div className="flex items-center gap-2.5">
          <AgentMark name={name} iconUrl={agent.icon_url} size={24} />
          <div className="flex min-w-0 items-baseline gap-2">
            <Link
              to="/agents/$agentId"
              params={{ agentId: agent.id }}
              search={{}}
              state={linkState}
              className="max-w-full shrink-0 truncate font-medium outline-none after:absolute after:inset-0 after:content-[''] focus-visible:after:ring-2 focus-visible:after:ring-ring focus-visible:after:ring-inset"
            >
              {name}
            </Link>
            <span className="truncate font-mono text-xs text-muted-foreground">{agent.name}</span>
          </div>
        </div>
        {reason ? (
          <p className="mt-1 line-clamp-1 text-xs font-medium" data-testid="health-reason">
            {copy.healthReason(reason)}
          </p>
        ) : null}
        <div className="mt-1 space-y-0.5 text-xs text-muted-foreground sm:hidden">
          <p className="truncate">{description}</p>
          <p className="truncate">{owner ? `${access} · ${owner}` : access}</p>
        </div>
      </TableCell>
      <TableCell>
        {harness ? null : (
          <StatusBadge display={displayStatus(agent.status, false)} raw={agent.status} />
        )}
      </TableCell>
      <TableCell className={cn(WIDE, 'max-w-0 text-muted-foreground')}>
        <p className="truncate" title={agent.description ?? undefined}>
          {description}
        </p>
      </TableCell>
      <TableCell className={SKILLS}>
        <div className="flex gap-1">
          {skills.slice(0, ROW_SKILLS).map((s, i) => (
            <Badge key={s.id} variant="secondary" className={cn('font-normal', i > 0 && ALL_CHIPS)}>
              {s.name}
            </Badge>
          ))}
          {more(1, ONE_CHIP)}
          {more(ROW_SKILLS, ALL_CHIPS)}
        </div>
      </TableCell>
      <TableCell className={cn(WIDE, 'pr-4 text-xs text-muted-foreground')}>
        <Badge variant="outline">{access}</Badge>
        {owner ? <span className="ml-2">{owner}</span> : null}
      </TableCell>
    </TableRow>
  )
})
