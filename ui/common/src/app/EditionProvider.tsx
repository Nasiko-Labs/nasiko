import { useMemo, type ReactNode } from 'react'
import { resolveSlots, type Edition } from './edition'
import { EditionContext } from './edition-context'

export function EditionProvider({ edition, children }: { edition: Edition; children: ReactNode }) {
  const value = useMemo(() => ({ edition, slots: resolveSlots(edition.layers) }), [edition])
  return <EditionContext value={value}>{children}</EditionContext>
}
