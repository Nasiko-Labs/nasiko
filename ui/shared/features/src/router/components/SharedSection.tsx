/**
 * Shared with you: the agents the viewer can use but does not own (the route answers 400 for an owner), each with a
 * select of the viewer's own configs. Hidden while there is none. Only the first SHARED_VISIBLE rows exist, hence are
 * read, until Show all.
 */
import { useQuery } from '@tanstack/react-query'
import { useMemo, useState } from 'react'
import { fmtInt } from '@nasiko/ui/lib/format'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { directoryQuery } from '@nasiko/features/agents/api'
import { AgentLinkTo } from '@nasiko/features/agents/components/bits'
import type { Agent } from '@nasiko/features/agents/types'
import { mineQuery, useSetMyConfig } from '../api'
import { copy } from '../copy'
import { routerError } from '../errors'
import { SHARED_VISIBLE } from '../tuning'
import type { LlmConfig } from '../types'
import { SectionError } from './SectionError'

/** The select's value for "no choice"; a config id is a UUID, so it cannot collide. */
const DEFAULT = 'default'

// Not in copy.ts: that module is in an app's shell chunk, whose size budget has no room left.
const text = {
  title: 'Shared with you',
  help: 'Pick one of your models for an agent someone shared with you. Calls then use your key. Left on Default, calls use your default model, unless the agent’s owner covers them.',
  default: 'Default',
  pick: (name: string) => `Your model for ${name}`,
  listFailed: 'Couldn’t list the agents shared with you',
  showAll: (n: string) => `Show all ${n}`,
}

export function SharedSection({
  viewer,
  configs,
}: {
  viewer: string
  /** The viewer's own configs; undefined while they could not be read. */
  configs: LlmConfig[] | undefined
}) {
  const directory = useQuery(directoryQuery)
  const agents = useMemo(
    () =>
      (directory.data ?? [])
        .filter((a) => a.owner_id !== viewer)
        .sort((a, b) => (a.display_name || a.name).localeCompare(b.display_name || b.name)),
    [directory.data, viewer],
  )
  const [all, setAll] = useState(false)
  const failed = directory.isError && !directory.data
  if (!failed && !agents.length) return null
  const capped = !all && agents.length > SHARED_VISIBLE
  return (
    <section aria-labelledby="router-shared-h" className="mt-6 space-y-2">
      <div>
        <h2 id="router-shared-h" className="text-sm font-medium">
          {text.title}
        </h2>
        <p className="text-xs text-muted-foreground">{text.help}</p>
      </div>
      {failed ? (
        <SectionError
          title={text.listFailed}
          error={directory.error}
          onRetry={() => void directory.refetch()}
        />
      ) : (
        <ul
          aria-label={text.title}
          className="divide-y divide-border rounded-lg border border-border bg-card"
        >
          {(capped ? agents.slice(0, SHARED_VISIBLE) : agents).map((a) => (
            <SharedRow key={a.id} agent={a} configs={configs ?? []} />
          ))}
        </ul>
      )}
      {capped ? (
        <Button
          size="sm"
          variant="outline"
          className="pointer-coarse:min-h-11"
          onClick={() => setAll(true)}
        >
          {text.showAll(fmtInt(agents.length))}
        </Button>
      ) : null}
    </section>
  )
}

function SharedRow({ agent, configs }: { agent: Agent; configs: LlmConfig[] }) {
  const name = agent.display_name || agent.name
  const read = useQuery(mineQuery(agent.id))
  const write = useSetMyConfig(agent.id)
  const chosen = read.data?.llm_config_id
  // A choice of a config deleted since this row was read is no choice, as the server reads it.
  const stored = chosen && configs.some((c) => c.id === chosen) ? chosen : DEFAULT
  // While a write runs the pick stays shown; a failed write leaves the stored value showing.
  const value = write.isPending ? (write.variables ?? DEFAULT) : stored
  return (
    <li
      data-agent={agent.id}
      className="flex flex-wrap items-center gap-x-3 gap-y-1 px-4 py-2.5 text-sm"
    >
      <AgentLinkTo id={agent.id} className="min-w-0 flex-1 truncate font-medium">
        {name}
      </AgentLinkTo>
      {read.isError ? (
        <>
          <span className="text-destructive">{copy.couldntRead}</span>
          <Button
            size="sm"
            variant="outline"
            className="h-7 pointer-coarse:min-h-11"
            onClick={() => void read.refetch()}
          >
            {copy.retry}
          </Button>
        </>
      ) : read.isPending ? (
        <span className="text-muted-foreground">{copy.readingRouting}</span>
      ) : (
        <Select
          value={value}
          disabled={write.isPending}
          onValueChange={(v) => write.mutate(v === DEFAULT ? null : v)}
        >
          <SelectTrigger size="sm" aria-label={text.pick(name)} className="pointer-coarse:min-h-11">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={DEFAULT}>{text.default}</SelectItem>
            {configs.map((c) => (
              <SelectItem key={c.id} value={c.id}>
                {c.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {write.isError ? (
        <p role="alert" className="basis-full text-xs text-destructive">
          {routerError(write.error).problem}
        </p>
      ) : null}
    </li>
  )
}
