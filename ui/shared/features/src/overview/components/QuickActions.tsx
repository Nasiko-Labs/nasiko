/**
 * Quick actions (plans/feat-overview.md §9): New chat, Ask the Orchestrator, Deploy an agent (plans/feat-deploy.md §7),
 * Adjust budget when the server has budgets, and the deploy command with Copy as the CLI alternative. The buttons sit
 * side by side once the card is wide enough (it spans the row in some layouts). The first run has its own panel
 * (SetUpNow.tsx).
 */
import { Link } from '@tanstack/react-router'
import { Gauge, MessageSquare, Route as RouteIcon } from 'lucide-react'
import { CopyButton } from '@nasiko/ui/composites/copy-button'
import { asButton } from '@nasiko/ui/primitives/button'
import { Separator } from '@nasiko/ui/primitives/separator'
import { DEPLOY_CMD } from '@nasiko/features/agents/format'
import { DeployAgentButton } from '@nasiko/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@nasiko/features/deploy/copy'
import { copy } from '../copy'
import { Card, TOUCH } from './Card'

export function QuickActions({
  budgets = false,
  className,
}: {
  budgets?: boolean
  className?: string
}) {
  return (
    <Card id="overview-actions" title={copy.actions.title} className={className}>
      <div className="flex flex-col gap-2">
        {/* Stacked beside Recent chats (from 1100 px), side by side when the card spans the row. */}
        <div className="grid grid-cols-[repeat(auto-fit,minmax(12rem,1fr))] gap-2 @[1100px]/overview:grid-cols-1">
          <Link to="/chat" {...asButton({ className: `justify-start ${TOUCH}` })}>
            <MessageSquare aria-hidden /> {copy.actions.newChat}
          </Link>
          <Link
            to="/chat"
            search={{ auto: 1 } as never}
            {...asButton({ variant: 'outline', className: `justify-start ${TOUCH}` })}
          >
            <RouteIcon aria-hidden /> {copy.actions.orchestrator}
          </Link>
          <DeployAgentButton
            variant="outline"
            size="default"
            className={`justify-start ${TOUCH}`}
          />
          {budgets ? (
            <Link
              to="/router"
              hash="router-budgets"
              {...asButton({ variant: 'outline', className: `justify-start ${TOUCH}` })}
            >
              <Gauge aria-hidden /> {copy.actions.adjustBudget}
            </Link>
          ) : null}
        </div>
        <Separator className="my-1" />
        <div className="flex flex-col gap-1">
          <span className="text-xs text-muted-foreground">{deployCopy.entry.orCli}</span>
          <CopyButton text={DEPLOY_CMD} label={copy.actions.copyDeploy} showText />
        </div>
      </div>
    </Card>
  )
}
