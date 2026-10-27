/**
 * The map's filters: team, owner (who is on a problem, or Nobody), kind of problem and status. They dim what doesn't
 * match on the map and narrow the insight panel; Clear filters puts everything back. Search params, replaced.
 */
import { Button } from '@nasiko/ui/primitives/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { cn } from '@nasiko/ui/lib/utils'
import { estateCopy as copy } from '../estate/copy'
import type { EstateView } from '../estate/useEstateView'
import type { OverviewSearch } from '../search'

const ALL = '__all'

type Key = 'team' | 'owner' | 'kind' | 'tier'

function Filter({
  label,
  value,
  all,
  options,
  onChange,
}: {
  label: string
  value: string | undefined
  all: string
  options: [string, string][]
  onChange(v: string | undefined): void
}) {
  return (
    <Select value={value ?? ALL} onValueChange={(x) => onChange(x === ALL ? undefined : x)}>
      <SelectTrigger
        size="sm"
        aria-label={label}
        className={cn('h-8 gap-1.5 text-xs', value && 'border-foreground/40')}
      >
        <span className="text-muted-foreground">{label}</span>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={ALL}>{all}</SelectItem>
        {options.map(([v, l]) => (
          <SelectItem key={v} value={v}>
            {l}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

export function FilterBar({
  v,
  search,
  me,
  onChange,
}: {
  v: EstateView
  search: OverviewSearch
  me: string
  onChange(patch: Partial<Pick<OverviewSearch, Key>>): void
}) {
  const owners = [
    ...new Set(
      v.estate.problems.map((p) => p.owner_id).filter((x): x is string => !!x && x !== me),
    ),
  ]
    .map((id) => [id, v.nodes.get(id)?.name ?? id] as [string, string])
    .sort((a, b) => a[1].localeCompare(b[1]))
  const any = !!(search.team || search.owner || search.kind || search.tier)
  return (
    <div
      className="flex flex-wrap items-center gap-2"
      role="group"
      aria-label={copy.filters.problem}
    >
      {v.estate.groups.length ? (
        <Filter
          label={copy.filters.team}
          value={search.team}
          all={copy.filters.allTeams}
          options={v.estate.groups.map((g) => [g.id, g.name])}
          onChange={(team) => onChange({ team })}
        />
      ) : null}
      <Filter
        label={copy.filters.owner}
        value={search.owner}
        all={copy.filters.anyone}
        options={[[me, copy.filters.me], ...owners, ['none', copy.filters.nobody]]}
        onChange={(owner) => onChange({ owner })}
      />
      <Filter
        label={copy.filters.problem}
        value={search.kind}
        all={copy.filters.allKinds}
        options={[
          ['errors', copy.kind.errors],
          ['slow', copy.kind.slow],
          ['cost', copy.kind.cost],
        ]}
        onChange={(kind) => onChange({ kind: kind as OverviewSearch['kind'] })}
      />
      <Filter
        label={copy.filters.status}
        value={search.tier}
        all={copy.filters.any}
        options={[
          ['broken', copy.tier.broken],
          ['worse', copy.tier.worse],
        ]}
        onChange={(tier) => onChange({ tier: tier as OverviewSearch['tier'] })}
      />
      {any ? (
        <Button
          variant="link"
          size="sm"
          className="h-auto px-1 text-xs text-muted-foreground"
          onClick={() =>
            onChange({ team: undefined, owner: undefined, kind: undefined, tier: undefined })
          }
        >
          {copy.filters.clear}
        </Button>
      ) : null}
    </div>
  )
}
