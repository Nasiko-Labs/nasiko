/**
 * Guide step 5: the optimisation switches, and where to find them.
 *
 * It exists because nothing else in the product tells a new user these are there — they live one
 * tab deep on an agent, every one is off by default, and the difference between having them on and
 * not is a materially different bill. A feature nobody discovers may as well not ship.
 *
 * Informational rather than a form. The switches are per agent, and at this point in the guide the
 * agent from the previous step is usually still building, so offering a toggle here would either
 * be disabled or write to something that does not exist yet. It explains and points instead.
 */
import { Scissors } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { StepHeading } from './parts'
import { copy as guideCopy } from './copy'

const copy = guideCopy.optimise

export function OptimiseStep({ onOpenAgents }: { onOpenAgents: () => void }) {
  return (
    <div className="flex flex-col gap-6">
      <StepHeading title={copy.title} intro={copy.intro} />

      <ul className="flex flex-col gap-3">
        {copy.items.map((item) => (
          <li key={item.title} className="flex gap-3 rounded-xl border bg-card p-4">
            <span
              aria-hidden
              className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground"
            >
              <Scissors className="size-4" />
            </span>
            <div className="min-w-0">
              <p className="text-sm font-medium">{item.title}</p>
              <p className="mt-0.5 text-sm text-muted-foreground">{item.line}</p>
              {/* The path, not a link: these are per-agent settings and the guide does not yet know
                  which agent. Telling someone where to look beats sending them somewhere generic. */}
              <p className="mt-1 text-xs text-muted-foreground">{item.where}</p>
            </div>
          </li>
        ))}
      </ul>

      <p className="rounded-lg border border-dashed bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
        {copy.beta}
      </p>

      <Button type="button" variant="outline" className="self-start" onClick={onOpenAgents}>
        {copy.cta}
      </Button>
    </div>
  )
}
