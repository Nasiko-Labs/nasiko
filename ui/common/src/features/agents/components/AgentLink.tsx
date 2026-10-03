/**
 * E2: an agent name elsewhere in the app links to its detail page. Resolves through the shared
 * directory by id first, then by raw name; a name several agents share, or an agent the
 * directory doesn't have (deleted, not visible, still loading), renders plain text.
 */
import type { ReactNode } from 'react'
import { useAgentsDirectory } from '../api'
import { AgentLinkTo } from './bits'

/** `fallback` replaces the plain-text form (null hides it, for a link that only makes sense as a link). */
export function AgentLink({
  id,
  name,
  children,
  className,
  fallback,
}: {
  id?: string | null
  name?: string | null
  children: ReactNode
  className?: string
  fallback?: ReactNode
}) {
  const dir = useAgentsDirectory()
  const byName = name ? dir.byNameAll.get(name) : undefined
  const target =
    (id ? dir.byId.get(id)?.id : undefined) ?? (byName?.length === 1 ? byName[0]?.id : undefined)
  if (!target)
    return fallback !== undefined ? fallback : <span className={className}>{children}</span>
  return (
    <AgentLinkTo id={target} className={className}>
      {children}
    </AgentLinkTo>
  )
}
