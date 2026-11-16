/**
 * The Deploy page's model-key notice (user request 2026-11-16), atop each method's side column: an agent the user deploys
 * calls its model with the user's own key, set up in the LLM router. Where the Starter pack is offered, the layer's
 * `deployNote` replaces that line (MT: the Free plan's daily allowance for deployed agents, and when the user's key
 * takes over).
 */
import { Link } from '@tanstack/react-router'
import { KeyRound } from 'lucide-react'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { Alert, AlertDescription, AlertTitle } from '@nasiko/ui/primitives/alert'
import { asButton } from '@nasiko/ui/primitives/button'
import { useStarterPackOffered } from '@nasiko/features/onboarding/api'
import { copy } from '../copy'

export function ModelKeyNotice() {
  const deployNote = useSlots().starterPack?.deployNote
  const offered = useStarterPackOffered()
  return (
    // A standing note, not news: `role="alert"` would be read out on every visit. Tinted as the Build page's warnings
    // are, so it reads as something to do before the first run.
    <Alert role="note" className="border-warning/40 bg-warning/5 [&>svg]:text-warning">
      <KeyRound aria-hidden />
      <AlertTitle className="line-clamp-none">{copy.keyNotice.title}</AlertTitle>
      <AlertDescription>
        <p>{offered && deployNote ? deployNote : copy.keyNotice.body}</p>
        <Link to="/router" {...asButton({ variant: 'link', size: 'sm', className: 'h-auto px-0' })}>
          {copy.keyNotice.link}
        </Link>
      </AlertDescription>
    </Alert>
  )
}
