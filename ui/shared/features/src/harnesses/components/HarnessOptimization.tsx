/**
 * Token optimization for the viewer's coding harnesses, on the import card once they are in: the agent Settings switch
 * (agents/detail/TokenOptimization.tsx) as one row, over every harness that has an LLM config attached. Those are the
 * ones whose sessions go through Nasiko's LLM router, which is where the optimization runs (it compresses their tool
 * output, `compress_enabled`); a harness without one is left out, and with none the switch is disabled and says how
 * to attach one. On writes `compress_enabled: true` to each included harness that has it off, off writes `false` to
 * each one that has it on (`PUT /api/agents/{id}`, the bulk write the Optimization page uses), then the re-read lists
 * decide. Saved on flip, like the agent's; failures toast with the server's reason. Nothing renders until the viewer
 * owns a harness.
 */
import { useQuery } from '@tanstack/react-query'
import { useId } from 'react'
import { toast } from 'sonner'
import { BetaBadge } from '@nasiko/ui/composites/beta-badge'
import { Field, FieldContent, FieldDescription, FieldLabel } from '@nasiko/ui/primitives/field'
import { Switch } from '@nasiko/ui/primitives/switch'
import { useOwnedAgents, useSetCompressionFor } from '@nasiko/features/agents/api'
import { copy as agentsCopy } from '@nasiko/features/agents/copy'
import { isHarness } from '@nasiko/features/agents/status'
import { useRowReads } from '@nasiko/features/router/api'
import { meQuery } from '@nasiko/data-access/auth'
import { ApiError } from '@nasiko/data-access/client'
import { copy as pageCopy } from '../copy'
import { attachedHarnesses } from '../routing'

const copy = pageCopy.optimization

export function HarnessOptimization() {
  const sub = useQuery(meQuery).data?.sub
  const owned = useOwnedAgents(sub)
  const set = useSetCompressionFor()
  const id = useId()
  const harnesses = (owned.data ?? []).filter(isHarness)
  const { reads } = useRowReads(harnesses.map((a) => a.id))
  if (!harnesses.length) return null
  const { included, loading } = attachedHarnesses(harnesses, reads)
  const on = included.filter((a) => a.compress_enabled === true)
  const left = harnesses.length - included.length
  // While a save runs the switch shows what was asked; after it the re-read list decides.
  const checked =
    set.isPending && set.variables
      ? set.variables.enabled
      : included.length > 0 && on.length === included.length
  const unavailable = !loading && included.length === 0
  const flip = (next: boolean) => {
    if (set.isPending || !included.length) return
    const ids = included.filter((a) => (a.compress_enabled === true) !== next).map((a) => a.id)
    set.mutate(
      { ids, enabled: next },
      {
        onSuccess: ({ failed }) => {
          const first = failed[0]?.error
          if (first !== undefined) toast.error(copy.failed(failed.length, reasonOf(first)))
        },
        onError: (e) => toast.error(copy.failed(ids.length, reasonOf(e))),
      },
    )
  }
  return (
    // A row at the foot of the import card, a hairline above it: the step after the agents came in.
    <Field
      orientation="horizontal"
      className="gap-6 border-t pt-4"
      data-disabled={unavailable || undefined}
    >
      <FieldContent>
        <FieldLabel htmlFor={id} className="gap-2">
          {agentsCopy.tokenOptimization}
          <BetaBadge />
        </FieldLabel>
        <FieldDescription id={`${id}-hint`}>
          {unavailable ? copy.needsConfig : copy.hint}
          {/* Mixed: the switch reads off, so say how many are on. */}
          {on.length > 0 && on.length < included.length
            ? ` ${copy.some(on.length, included.length)}`
            : null}
          {!unavailable && !loading && left > 0 ? ` ${copy.notAttached(left)}` : null}
        </FieldDescription>
      </FieldContent>
      <Switch
        id={id}
        checked={checked}
        aria-label={copy.label}
        aria-describedby={`${id}-hint`}
        // Disabled only when there is nothing it could turn on. Not while saving: that would drop keyboard focus
        // mid-save, so extra flips wait for the save instead.
        disabled={unavailable || loading}
        aria-busy={set.isPending}
        onCheckedChange={flip}
      />
    </Field>
  )
}

const reasonOf = (e: unknown) =>
  (e instanceof ApiError && e.serverMessage) || (e instanceof Error ? e.message : String(e))
