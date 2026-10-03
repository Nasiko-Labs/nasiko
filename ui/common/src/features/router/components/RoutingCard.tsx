/**
 * The agent detail's routing card (plan §4.5, Overview under the status block): the routing sentence, the key
 * source and "Change routing". The read is enabled only for the owner or a superuser (eng #9); a 403 or any other
 * failed read hides the card (never the 401 path, which the query client owns). Non-owners see nothing.
 */
import { useQuery } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { meQuery } from '@/lib/api/auth'
import { useAgentRouting, useCatalog, useConfigs } from '../api'
import { copy } from '../copy'
import { keySource, routingSentence } from '../routing'
import { Announcer, KeySourceChip, SentenceText, SourceBadge } from './bits'
import { RoutingSheet } from './RoutingSheet'

export function RoutingCard({
  agent,
}: {
  agent: { id: string; name: string; displayName: string; ownerId: string }
}) {
  const me = useQuery(meQuery).data
  const superuser = !!me?.is_superuser
  const owner = !!me?.sub && me.sub === agent.ownerId
  const routing = useAgentRouting(agent.id, owner || superuser)
  if (!owner && !superuser) return null
  if (routing.isError) return null
  return (
    <Announcer>
      <Card
        agent={agent}
        owner={owner}
        superuser={superuser}
        sub={me?.sub}
        routing={routing.data}
      />
    </Announcer>
  )
}

function Card({
  agent,
  owner,
  superuser,
  sub,
  routing,
}: {
  agent: { id: string; name: string; displayName: string; ownerId: string }
  owner: boolean
  superuser: boolean
  sub?: string
  routing: ReturnType<typeof useAgentRouting>['data']
}) {
  const [open, setOpen] = useState(false)
  // Configs are the viewer's own, so they only help when the viewer owns the agent; both only matter once the sheet opens.
  const configs = useConfigs(owner && open)
  const catalog = useCatalog(open)
  return (
    <section
      aria-labelledby={`routing-${agent.id}`}
      className="space-y-2 rounded-lg border border-border bg-card p-4 text-sm"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id={`routing-${agent.id}`} className="font-semibold">
          {copy.cardTitle}
        </h2>
        <Link
          to="/router"
          className="text-xs text-muted-foreground underline-offset-4 hover:underline pointer-coarse:min-h-11"
        >
          {copy.cardOpenRouter}
        </Link>
      </div>
      {!routing ? (
        <Skeleton className="h-10" />
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <SourceBadge source={routing.source} viewerIsOwner={owner} />
          <SentenceText
            sentence={routingSentence(routing.llm_config, routing.pinned_model)}
            full
            className="min-w-0"
          />
          <KeySourceChip source={keySource(routing.llm_config)} />
          <Button
            size="sm"
            variant="ghost"
            className="ml-auto h-8 px-2 text-primary-text pointer-coarse:min-h-11"
            onClick={() => setOpen(true)}
          >
            {copy.changeRouting}
          </Button>
        </div>
      )}
      <RoutingSheet
        target={open ? agent : null}
        viewer={{ sub, superuser }}
        configs={owner ? configs.data : undefined}
        catalog={catalog.data}
        onClose={() => setOpen(false)}
        onSaved={() => {}}
      />
    </section>
  )
}
