"use client";

import {
  AlertCircleIcon,
  BotIcon,
  CheckCircle2Icon,
  CircleDashedIcon,
  FileIcon,
  ShieldCheckIcon,
  UserIcon,
  WrenchIcon,
} from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "../lib/cn";
import type { ActivityEvent } from "@nasiko/contracts";
import { ACTIVITY_PAGE_MAX } from "@nasiko/contracts/limits";
import { Button } from "../primitives/button";
import { useMotion } from "../motion/provider";
import { SafeLink } from "./safe-link";

/**
 * ActivityTimeline shows declared progress, tool events and explanations the
 * backend actually emitted (contract §7). It renders only `ActivityEvent`s it
 * was given: no synthetic "thinking…" rows, no percentages, no animated steps.
 * The newest event gets one brief emphasis (§8.3 timeline: "brief emphasis on
 * actual new event"); the list is bounded and offers "earlier" on demand.
 */
export interface ActivityTimelineProps {
  /** Newest last. */
  readonly events: readonly ActivityEvent[];
  readonly initialVisible?: number;
  readonly className?: string;
  readonly label?: string;
  readonly formatTime?: (iso: string) => string;
}

const ICON: Record<ActivityEvent["kind"], typeof BotIcon> = {
  message: BotIcon,
  "tool.start": WrenchIcon,
  "tool.result": CheckCircle2Icon,
  "tool.error": AlertCircleIcon,
  "approval.requested": ShieldCheckIcon,
  "approval.decided": ShieldCheckIcon,
  status: CircleDashedIcon,
  artifact: FileIcon,
};

/**
 * Emphasis for an event that arrived after the timeline mounted: on at mount,
 * off after a bounded time. Events present at first render never flash
 * (history is restored, not replayed — §8.3 chat history).
 */
function NewEventEmphasis({ isNew, children }: { isNew: boolean; children: ReactNode }) {
  const { recipe } = useMotion();
  const r = recipe("emphasis");
  const [active, setActive] = useState(isNew && r.durationMs > 0);
  useEffect(() => {
    if (!active) return undefined;
    const t = setTimeout(() => {
      setActive(false);
    }, r.durationMs * 4);
    return () => {
      clearTimeout(t);
    };
  }, [active, r.durationMs]);
  return (
    <div
      data-emphasis={active ? "on" : "off"}
      className="rounded-sm transition-colors duration-micro ease-standard data-[emphasis=on]:bg-info/15"
    >
      {children}
    </div>
  );
}

export function ActivityTimeline({
  events,
  initialVisible = 20,
  className,
  label = "Agent activity",
  formatTime = (iso) => iso,
}: ActivityTimelineProps) {
  const bounded = events.slice(-ACTIVITY_PAGE_MAX);
  const [showAll, setShowAll] = useState(false);
  const visible = showAll ? bounded : bounded.slice(-initialVisible);
  const hidden = bounded.length - visible.length;
  const initialIds = useRef<ReadonlySet<string> | null>(null);
  const seenAtMount = (initialIds.current ??= new Set(bounded.map((e) => e.id)));
  return (
    <section aria-label={label} className={cn("flex flex-col gap-2", className)}>
      {hidden > 0 ? (
        <Button
          variant="ghost"
          size="sm"
          className="self-start"
          onClick={() => {
            setShowAll(true);
          }}
        >
          Show {hidden} earlier
        </Button>
      ) : null}
      <ol
        className="relative flex flex-col gap-3 border-l border-border pl-4"
        aria-live="polite"
        aria-relevant="additions"
      >
        {visible.map((e) => {
          const Icon = e.actor === "user" ? UserIcon : ICON[e.kind];
          const tone =
            e.status === "error" || e.kind === "tool.error"
              ? "text-destructive"
              : e.status === "warning"
                ? "text-warning-foreground"
                : e.status === "running"
                  ? "text-info-foreground"
                  : "text-muted-foreground";
          return (
            <li
              key={e.id}
              className="relative text-sm"
              data-kind={e.kind}
              data-status={e.status ?? "none"}
            >
              <span
                className={cn(
                  "absolute -left-[25px] top-0.5 flex size-4 items-center justify-center rounded-full bg-background",
                  tone,
                )}
              >
                <Icon className="size-3.5" aria-hidden />
              </span>
              <NewEventEmphasis isNew={!seenAtMount.has(e.id)}>
                <div className="flex flex-col gap-0.5">
                  <div className="flex flex-wrap items-baseline gap-x-2">
                    <span className="text-xs text-muted-foreground">{e.actor}</span>
                    <time className="text-xs text-muted-foreground tabular-nums" dateTime={e.at}>
                      {formatTime(e.at)}
                    </time>
                    {e.status ? (
                      <span className={cn("text-xs", tone)} data-slot="status">
                        {e.status}
                      </span>
                    ) : null}
                  </div>
                  <p className="whitespace-pre-wrap break-words">{e.text}</p>
                  {e.links?.length ? (
                    <ul className="flex flex-wrap gap-3 text-xs">
                      {e.links.map((l) => (
                        <li key={`${l.href}:${l.label}`}>
                          <SafeLink href={l.href}>{l.label}</SafeLink>
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </div>
              </NewEventEmphasis>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
