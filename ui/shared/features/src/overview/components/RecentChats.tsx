// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/components/RecentChats.tsx. Local edits: the lab's `EmptyState` (shared/state-card) is `LabEmptyState`; `"use
// client"`; TanStack `Link` -> the navigation port's `Link` (`@nasiko/app-core/navigation`) (a chat's href is chat/nav.ts `chatHref`); `Button asChild` + `Link` ->
// `ButtonLink`; optional props admit `undefined`; Prettier formatting.
"use client";

/**
 * Recent chats: the viewer's newest chats (the chat rail's own list and cache, so Chat opens warm), each with who it
 * talks to, named by chat's `chatIdentity` exactly as the rail names it. Recorded harness chats stay in Chat's Recorded
 * view. The dot is the agent's status in the directory (the Orchestrator's is its own colour); the name beside it says who.
 */
import Link from "@nasiko/app-core/navigation";
import { useRef } from "react";
import { LabEmptyState as EmptyState } from "@nasiko/ui/composites";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { relTime } from "../../agents/format";
import { displayStatus } from "../../agents/status";
import type { Agent } from "../../agents/types";
import { sessionRows, useChatSessions } from "../../chat/api";
import { chatIdentity } from "../../chat/identity";
import { chatHref } from "../../chat/nav";
import { cn } from "@nasiko/ui/cn";
import { copy } from "../copy";
import { RECENT_SESSION_ROWS } from "../tuning";
import { Card, CardError, CardSkeleton, TOUCH } from "./Card";

const DOT = {
  running: "bg-success",
  deploying: "bg-info",
  attention: "bg-destructive",
} as Partial<Record<string, string>>;

export function RecentChats({
  byId,
  username,
  now,
  className,
}: {
  byId: ReadonlyMap<string, Agent>;
  username: string;
  now: number;
  className?: string | undefined;
}) {
  const titleRef = useRef<HTMLHeadingElement>(null);
  const chats = useChatSessions();
  const rows = sessionRows(chats.data)
    .map((r) => ({ row: r, who: chatIdentity(r, byId, username) }))
    .filter((c) => c.who.kind !== "recorded")
    .slice(0, RECENT_SESSION_ROWS);
  return (
    <Card
      id="overview-chats"
      title={copy.chats.title}
      to="/chat"
      linkLabel={copy.chats.link}
      titleRef={titleRef}
      className={className}
    >
      {chats.isPending ? (
        <CardSkeleton rows={5} />
      ) : chats.isError ? (
        <CardError
          what={copy.chats.what}
          onRetry={() => void chats.refetch()}
          titleRef={titleRef}
        />
      ) : !rows.length ? (
        <EmptyState
          title={copy.chats.none}
          action={
            <ButtonLink variant="outline" size="sm" className={TOUCH} href="/chat">
              {copy.chats.start}
            </ButtonLink>
          }
          className="py-6 md:py-6"
        />
      ) : (
        <ul className="divide-y divide-border">
          {rows.map(({ row, who }) => (
            <li
              key={row.session_id}
              data-testid="recent-chat"
              className="flex min-h-11 items-center gap-3 py-2 text-sm"
            >
              <span
                aria-hidden
                className={cn(
                  "size-2 shrink-0 rounded-full",
                  who.kind === "orchestrator"
                    ? "bg-primary"
                    : (DOT[displayStatus(who.status, false)] ?? "bg-muted-foreground/40"),
                )}
              />
              <Link
                href={chatHref(row.session_id)}
                className="min-w-0 flex-1 truncate font-medium underline-offset-4 hover:underline"
              >
                {row.title || copy.needs.untitledChat}
              </Link>
              <span className="hidden max-w-[40%] shrink-0 truncate text-xs text-muted-foreground @[360px]/card:inline">
                {who.name}
              </span>
              <span className="w-20 shrink-0 text-right text-xs text-muted-foreground tabular-nums">
                {relTime(row.updated_at ?? row.created_at, now)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
