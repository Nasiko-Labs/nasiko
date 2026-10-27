/**
 * Where a problem's, chain's or opportunity's one action goes, and each node's owning page: the server names a page
 * (`Target`), never a URL, and this is the one place that turns it into a link with its context kept.
 */
import { Link } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { withButtonStyle } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import type { Estate, Target } from '../estate/types'
import { LAST_WEEK } from '../tuning'

const isConnector = (e: Estate, id: string | null | undefined) =>
  !!id && e.tools.some((t) => t.id === id && t.connector_id)

export function TargetLink({
  target,
  estate,
  children,
  className,
  variant = 'default',
  size = 'sm',
  label,
}: {
  target: Target
  estate: Estate
  children: ReactNode
  className?: string
  variant?: 'default' | 'outline' | 'ghost' | 'link'
  size?: 'sm' | 'default'
  label?: string
}) {
  const q = target.query ?? {}
  const id = target.id ?? null
  const link = (() => {
    switch (target.page) {
      case 'agent':
        return id ? (
          <Link
            to="/agents/$agentId"
            params={{ agentId: id }}
            search={q as never}
            aria-label={label}
          >
            {children}
          </Link>
        ) : null
      case 'agents':
        return (
          <Link to="/agents" search={q as never} aria-label={label}>
            {children}
          </Link>
        )
      case 'builds':
        return (
          <Link to="/builds" search={q as never} aria-label={label}>
            {children}
          </Link>
        )
      case 'sessions':
        return (
          <Link to="/sessions" search={{ ...LAST_WEEK, ...q } as never} aria-label={label}>
            {children}
          </Link>
        )
      case 'tokenops':
        return (
          <Link to="/tokenops" search={q as never} aria-label={label}>
            {children}
          </Link>
        )
      case 'optimization':
        return (
          <Link to="/optimization" search={q as never} aria-label={label}>
            {children}
          </Link>
        )
      case 'router':
        return (
          <Link to="/router" aria-label={label}>
            {children}
          </Link>
        )
      case 'connector':
        return isConnector(estate, id) && id ? (
          <Link
            to="/mcp/$connectorId"
            params={{ connectorId: estate.tools.find((t) => t.id === id)?.connector_id ?? id }}
            search={{} as never}
            aria-label={label}
          >
            {children}
          </Link>
        ) : (
          <Link to="/mcp" search={{} as never} aria-label={label}>
            {children}
          </Link>
        )
      case 'mcp':
        return (
          <Link to="/mcp" search={q as never} aria-label={label}>
            {children}
          </Link>
        )
      case 'workflow':
        return id ? (
          <Link
            to="/workflows/$workflowId"
            params={{ workflowId: id }}
            search={{} as never}
            aria-label={label}
          >
            {children}
          </Link>
        ) : null
      case 'harnesses':
        return (
          <Link to="/harnesses" search={q as never} aria-label={label}>
            {children}
          </Link>
        )
      case 'chat':
        return (
          <Link to="/chat" aria-label={label}>
            {children}
          </Link>
        )
    }
  })()
  if (!link) return null
  return withButtonStyle(link, {
    variant: variant,
    size: size,
    className: cn('pointer-coarse:min-h-11', className),
  })
}
