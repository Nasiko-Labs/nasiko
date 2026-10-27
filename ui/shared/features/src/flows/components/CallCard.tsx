/**
 * A flow with one call and nothing under it (plans/feat-flows.md F12): the most common flow today, so it reads as a
 * call, never as a one-bar swimlane with an empty Steps table.
 */
import { Panel } from '@nasiko/ui/composites/panel'
import { copy } from '../copy'

export function CallCard({ agent, request }: { agent: string; request: string | null }) {
  return (
    <Panel
      title={copy.callCard.title}
      labelledBy="flow-call"
      subtitle={copy.callCard.direct(agent)}
    >
      {request ? (
        <dl className="grid gap-x-4 gap-y-2 text-sm sm:grid-cols-[8rem_1fr]">
          <dt className="text-muted-foreground">{copy.callCard.input}</dt>
          <dd className="whitespace-pre-wrap">{request}</dd>
        </dl>
      ) : null}
    </Panel>
  )
}
