/**
 * Quick actions (plans/feat-overview.md §9): New chat, Ask the Orchestrator, Deploy an agent (plans/feat-deploy.md §7),
 * Adjust budget when the server has budgets, and the deploy command with Copy as the CLI alternative. The buttons sit
 * side by side once the card is wide enough (it spans the row in some layouts). On first run Deploy leads the page, so
 * this card keeps only the chats, all outline: the page has one primary action.
 */
import { Link } from '@tanstack/react-router'
import { Gauge, MessageSquare, Route as RouteIcon } from 'lucide-react'
import { CopyButton } from '@/components/shared/copy-button'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { DEPLOY_CMD } from '@/features/agents/format'
import { DeployAgentButton } from '@/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@/features/deploy/copy'
import { copy } from '../copy'
import { Card, TOUCH } from './Card'

export function QuickActions({
  budgets = false,
  firstRun = false,
  className,
}: {
  budgets?: boolean
  firstRun?: boolean
  className?: string
}) {
  if (firstRun)
    return (
      <Card id="overview-actions" title={copy.actions.title} className={className}>
        {/* Stacked in the first run's narrow column (from 1100 px), side by side when the card spans the row. */}
        <div className="grid grid-cols-[repeat(auto-fit,minmax(12rem,1fr))] gap-2 @[1100px]/overview:grid-cols-1">
          <Button asChild variant="outline" className={`justify-start ${TOUCH}`}>
            <Link to="/chat">
              <MessageSquare aria-hidden /> {copy.actions.newChat}
            </Link>
          </Button>
          <Button asChild variant="outline" className={`justify-start ${TOUCH}`}>
            <Link to="/chat" search={{ auto: 1 } as never}>
              <RouteIcon aria-hidden /> {copy.actions.orchestrator}
            </Link>
          </Button>
        </div>
      </Card>
    )
  return (
    <Card id="overview-actions" title={copy.actions.title} className={className}>
      <div className="flex flex-col gap-2">
        <div className="grid grid-cols-[repeat(auto-fit,minmax(12rem,1fr))] gap-2">
          <Button asChild className={`justify-start ${TOUCH}`}>
            <Link to="/chat">
              <MessageSquare aria-hidden /> {copy.actions.newChat}
            </Link>
          </Button>
          <Button asChild variant="outline" className={`justify-start ${TOUCH}`}>
            <Link to="/chat" search={{ auto: 1 } as never}>
              <RouteIcon aria-hidden /> {copy.actions.orchestrator}
            </Link>
          </Button>
          <DeployAgentButton
            variant="outline"
            size="default"
            className={`justify-start ${TOUCH}`}
          />
          {budgets ? (
            <Button asChild variant="outline" className={`justify-start ${TOUCH}`}>
              <Link to="/router" hash="router-budgets">
                <Gauge aria-hidden /> {copy.actions.adjustBudget}
              </Link>
            </Button>
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
