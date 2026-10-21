// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/components/QuickActions.tsx. Local edits: `Button
// asChild` + TanStack `Link` -> `ButtonLink` (`/chat`, `/chat?auto=1`, the router's budgets hash through `pageHref`);
// optional props admit `undefined`; Prettier formatting.
// The redesign of TokenOps, Optimization, Overview and Harnesses (legacy ui/ on development, 3aac84f66: 8bf09bf29),
// merged into this port.
"use client";

/**
 * Quick actions (plans/feat-overview.md §9): New chat, Ask the Orchestrator, Deploy an agent (plans/feat-deploy.md §7),
 * Adjust budget when the server has budgets, and the deploy command with Copy as the CLI alternative. The buttons sit
 * side by side once the card is wide enough (it spans the row in some layouts). On first run Deploy leads the page, so
 * this card keeps only the chats, all outline: the page has one primary action.
 */
import { Gauge, MessageSquare, Route as RouteIcon } from "lucide-react";
import { CopyButton } from "@nasiko/ui/composites";
import { Separator } from "@nasiko/ui/primitives";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { DEPLOY_CMD } from "../../agents/format";
import { DeployAgentButton } from "../../deploy/components/DeployAgentButton";
import { copy as deployCopy } from "../../deploy/copy";
import { copy } from "../copy";
import { Card, pageHref, TOUCH } from "./Card";

export function QuickActions({
  budgets = false,
  firstRun = false,
  className,
}: {
  budgets?: boolean | undefined;
  firstRun?: boolean | undefined;
  className?: string | undefined;
}) {
  if (firstRun)
    return (
      <Card id="overview-actions" title={copy.actions.title} className={className}>
        {/* Stacked in the first run's narrow column (from 1100 px), side by side when the card spans the row. */}
        <div className="grid grid-cols-[repeat(auto-fit,minmax(12rem,1fr))] gap-2 @[1100px]/overview:grid-cols-1">
          <ButtonLink variant="outline" className={`justify-start ${TOUCH}`} href="/chat">
            <MessageSquare aria-hidden /> {copy.actions.newChat}
          </ButtonLink>
          <ButtonLink variant="outline" className={`justify-start ${TOUCH}`} href="/chat?auto=1">
            <RouteIcon aria-hidden /> {copy.actions.orchestrator}
          </ButtonLink>
        </div>
      </Card>
    );
  return (
    <Card id="overview-actions" title={copy.actions.title} className={className}>
      <div className="flex flex-col gap-2">
        {/* Stacked beside Recent chats (from 1100 px), side by side when the card spans the row. */}
        <div className="grid grid-cols-[repeat(auto-fit,minmax(12rem,1fr))] gap-2 @[1100px]/overview:grid-cols-1">
          <ButtonLink className={`justify-start ${TOUCH}`} href="/chat">
            <MessageSquare aria-hidden /> {copy.actions.newChat}
          </ButtonLink>
          <ButtonLink variant="outline" className={`justify-start ${TOUCH}`} href="/chat?auto=1">
            <RouteIcon aria-hidden /> {copy.actions.orchestrator}
          </ButtonLink>
          <DeployAgentButton
            variant="outline"
            size="default"
            className={`justify-start ${TOUCH}`}
          />
          {budgets ? (
            <ButtonLink
              variant="outline"
              className={`justify-start ${TOUCH}`}
              href={pageHref("/router", {}, "router-budgets")}
            >
              <Gauge aria-hidden /> {copy.actions.adjustBudget}
            </ButtonLink>
          ) : null}
        </div>
        <Separator className="my-1" />
        <div className="flex flex-col gap-1">
          <span className="text-xs text-muted-foreground">{deployCopy.entry.orCli}</span>
          <CopyButton text={DEPLOY_CMD} label={copy.actions.copyDeploy} showText />
        </div>
      </div>
    </Card>
  );
}
