import { useId } from 'react'
import { toast } from 'sonner'
import { BetaBadge } from '@nasiko/ui/composites/beta-badge'
import { Field, FieldContent, FieldDescription, FieldLabel } from '@nasiko/ui/primitives/field'
import { Switch } from '@nasiko/ui/primitives/switch'
import { ApiError } from '@nasiko/data-access/client'
import { useSetCompression } from '../api'
import { Section } from '../components/bits'
import { copy } from '../copy'
import type { AgentView } from '../normalize'

/**
 * Token optimization (plans/feat-context-optimization.md eng E1): one switch, saved on flip (the MCP Enabled switch's
 * model), optimistic, rolled back with a toast. On the Settings tab, and on a coding harness's Overview (it has no Settings tab),
 * since the owner's history compression counts every agent they own (eng E2, D10).
 */
export function TokenOptimization({ agent }: { agent: AgentView }) {
  const set = useSetCompression(agent.id, (e) =>
    toast.error(
      copy.tokenOptimizationFailed((e instanceof ApiError && e.serverMessage) || e.message),
    ),
  )
  const id = useId()
  // While a save runs the switch shows what was asked; after it (success or failure) the refetched detail decides.
  const on = set.isPending && set.variables !== undefined ? set.variables : agent.compressEnabled
  return (
    // Titled and badged as main's cloud-rs sync has every optimisation switch (Beta, one caveat; integration 2026-10-05).
    // Main's intro is the subtitle and the switch has its own label, so the title isn't printed twice; the row is the
    // Features rows' Field layout (review: design).
    <Section
      title={copy.tokenOptimization}
      subtitle={copy.tokenOptimizationIntro}
      action={<BetaBadge />}
    >
      <Field orientation="horizontal" className="gap-6">
        <FieldContent>
          <FieldLabel htmlFor={id}>{copy.tokenOptimizationSwitch}</FieldLabel>
          <FieldDescription id={`${id}-hint`}>
            {copy.tokenOptimizationHint}
            {agent.isHarness ? ` ${copy.tokenOptimizationHarness}` : null}
          </FieldDescription>
        </FieldContent>
        <Switch
          id={id}
          checked={on}
          aria-describedby={`${id}-hint`}
          // Not `disabled` while saving: that would drop keyboard focus mid-save. Extra flips wait for the save.
          aria-busy={set.isPending}
          onCheckedChange={(next) => {
            if (!set.isPending) set.mutate(next)
          }}
        />
      </Field>
    </Section>
  )
}
