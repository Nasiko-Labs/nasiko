// Ported from nasiko-ui-lab@1b99bdda common/src/features/chat/components/SessionHeader.tsx. Local edits: View session
// is a `ButtonLink` (the lab's `<Button asChild><Link>`); `DropdownMenuTrigger`/`TooltipTrigger asChild` are
// `render={…}` and the menu's View session item `render={<Link …/>}` (the navigation port's `Link` (`@nasiko/app-core/navigation`)) (Base UI useRender); Radix's
// `onSelect` on items is Base UI's `onClick` (the menu still closes on it); the menu's `onCloseAutoFocus` is Base UI's
// `finalFocus` (false while Rename takes focus); the totals label sits in a `DropdownMenuGroup` (Base UI's group label
// needs one); the jsx-a11y disable became a comment (no such rule here); `ApiError` is `ProductApiError`; Prettier
// formatting. Previous/Next (legacy ui/ on development, 3aac84f66: 858912f71, 12264f9b6, 1b7bfb943, fdd92df43):
// `ChatSiblings` pushes `chatHref(id, { mock, debug })` with the step's focus rule as history state (`chatFocus`).
/** An open chat's header (§7.3, §6.10): identity, title (inline rename), totals, View session and the ⋯ menu. */
import { Activity, MoreHorizontal } from "lucide-react";
import Link, { useRouter, useSearchParams } from "@nasiko/app-core/navigation";
import { siblingsAround } from "@nasiko/app-core/siblings";
import { PrevNext, type StepVia } from "@nasiko/ui/composites";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  Button,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  Input,
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@nasiko/ui/primitives";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { ProductApiError as ApiError } from "@nasiko/data-access/product";
import { useRenameChat } from "../api";
import { copy } from "../copy";
import { announce } from "../announce";
import { fmtCostPerOp, fmtTokens } from "@nasiko/ui/lib";
import { chatTotals } from "../format";
import { chatHref } from "../nav";
import { useRailOrder } from "../railOrder";
import type { ChatIdentity } from "../identity";
import type { ChatMessage, ChatSessionRow } from "../types";
import { IdentityChip, IdentityIcon } from "./ChatIdentity";
import { CHAT_COLUMN } from "./turnStyles";
import { cn } from "@nasiko/ui/cn";

// quirk: §10.11 — the server caps nothing; this is the UI's own cap.
const TITLE_MAX = 80;
const TITLE_COUNTER_FROM = 70;

export function SessionHeader({
  sessionId,
  row,
  loading,
  readOnly,
  identity,
  railButton,
  replies,
  partial,
  canExport,
  onExport,
  deleteBlocked,
  onDelete,
}: {
  sessionId: string;
  row: ChatSessionRow | undefined;
  /** The chat's details are still loading (the title says so). */
  loading: boolean;
  readOnly: boolean;
  identity: ChatIdentity | null;
  railButton: ReactNode;
  replies: ChatMessage[];
  /** Older pages exist: totals cover the loaded messages only. */
  partial: boolean;
  canExport: boolean;
  onExport: () => void;
  // quirk: §10.6 — delete fails on any request row (FK without cascade), so it is disabled up front.
  deleteBlocked: boolean;
  onDelete: () => void;
}) {
  const [editing, setEditing] = useState(false);
  /** Set while Rename is chosen from the menu, so the closing menu doesn't pull focus off the input. */
  const renaming = useRef(false);
  return (
    <header className="flex h-12 shrink-0 items-center border-b border-border">
      <div className={cn(CHAT_COLUMN, "flex min-w-0 items-center gap-2")}>
        {railButton}
        {identity ? <IdentityIcon kind={identity.kind} name={identity.name} size={20} /> : null}
        {editing && row ? (
          <RenameTitle
            sessionId={sessionId}
            title={row.title}
            onDone={() => {
              setEditing(false);
            }}
          />
        ) : (
          <Button
            type="button"
            variant="ghost"
            className="h-auto min-w-0 shrink justify-start px-0 py-0 text-left text-sm font-medium hover:bg-transparent hover:underline dark:hover:bg-transparent pointer-coarse:min-h-11"
            title={copy.rename}
            onClick={() => {
              if (row && !readOnly) setEditing(true);
            }}
          >
            <span className="truncate">
              {row?.title ?? (loading ? copy.loadingChat : copy.chatsNav)}
            </span>
          </Button>
        )}
        {/* The chip hides below 480 px of pane width; the icon keeps the kind (DS5). */}
        {identity ? (
          <IdentityChip identity={identity} className="hidden @min-[480px]:inline-flex" />
        ) : null}
        <div className="ml-auto flex shrink-0 items-center gap-1">
          <HeaderTotals replies={replies} partial={partial} className="hidden @min-[560px]:flex" />
          <ButtonLink
            href={`/sessions/${encodeURIComponent(sessionId)}`}
            size="sm"
            variant="ghost"
            className="hidden @min-[560px]:inline-flex pointer-coarse:min-h-11"
          >
            <Activity aria-hidden />
            {copy.viewSession}
          </ButtonLink>
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  size="icon-sm"
                  variant="ghost"
                  className="pointer-coarse:size-11"
                  aria-label={copy.moreChatActions}
                />
              }
            >
              <MoreHorizontal aria-hidden />
            </DropdownMenuTrigger>
            <DropdownMenuContent
              align="end"
              className="w-64"
              finalFocus={() => {
                // RenameTitle focuses itself; don't send focus back to the ⋯ trigger.
                if (!renaming.current) return true;
                renaming.current = false;
                return false;
              }}
            >
              {replies.length ? (
                <>
                  {/* Base UI's menu label belongs to a group (Radix's stood alone). */}
                  <DropdownMenuGroup>
                    <DropdownMenuLabel className="font-normal @min-[560px]:hidden">
                      <HeaderTotals replies={replies} partial={partial} stacked />
                    </DropdownMenuLabel>
                  </DropdownMenuGroup>
                  <DropdownMenuSeparator className="@min-[560px]:hidden" />
                </>
              ) : null}
              <DropdownMenuItem
                className="@min-[560px]:hidden pointer-coarse:min-h-11"
                render={<Link href={`/sessions/${encodeURIComponent(sessionId)}`} />}
              >
                {copy.viewSession}
              </DropdownMenuItem>
              <DropdownMenuItem
                className="pointer-coarse:min-h-11"
                disabled={!row || readOnly}
                onClick={() => {
                  renaming.current = true;
                  setEditing(true);
                }}
              >
                {copy.rename}
              </DropdownMenuItem>
              <DropdownMenuItem
                className="pointer-coarse:min-h-11"
                disabled={!canExport}
                onClick={onExport}
              >
                {copy.exportMarkdown}
              </DropdownMenuItem>
              <DropdownMenuItem
                disabled={deleteBlocked}
                className="text-destructive pointer-coarse:min-h-11"
                onClick={onDelete}
              >
                {deleteBlocked ? (
                  <span className="flex flex-col">
                    <span>{copy.delete}</span>
                    <span className="text-xs text-muted-foreground">{copy.deleteBlocked}</span>
                  </span>
                ) : (
                  copy.delete
                )}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          {/* Rightmost, so the arrows stay put from chat to chat whatever the totals' width. */}
          <ChatSiblings sessionId={sessionId} />
        </div>
      </div>
    </header>
  );
}

/**
 * Previous/Next through the rail's chats as shown (railOrder.ts). Pushes, like a rail click; dev aids carry over.
 * Below 480 px of pane width the buttons hide (the rail is the way there), but J/K still work: PrevNext stays mounted.
 */
function ChatSiblings({ sessionId }: { sessionId: string }) {
  const router = useRouter();
  const params = useSearchParams();
  const ids = useRailOrder((s) => s.ids);
  const around = siblingsAround(ids, sessionId);
  if (!around) return null;
  const go = (id: string | undefined, via: StepVia) => {
    if (!id) return;
    const href = chatHref(id, {
      mock: params.get("mock") ?? undefined,
      debug: params.get("debug") ?? undefined,
    });
    // A click lands in the composer; J/K keeps focus out of it, so the next key steps again.
    router.push(href, { scroll: false, state: { chatFocus: via === "key" ? "none" : "composer" } });
  };
  return (
    <PrevNext
      noun="chat"
      index={around.index}
      count={around.count}
      onPrev={(via) => {
        go(around.prev, via);
      }}
      onNext={(via) => {
        go(around.next, via);
      }}
      restoreFocus={false}
      className="hidden @min-[480px]:flex"
    />
  );
}

/**
 * Tokens and cost over the loaded replies (§7.3, E6): hidden before the first reply; labelled
 * "Usage for loaded messages" while older pages exist; unknown cost is "—", estimates get "~".
 */
function HeaderTotals({
  replies,
  partial,
  className,
  stacked = false,
}: {
  replies: ChatMessage[];
  partial: boolean;
  className?: string | undefined;
  stacked?: boolean | undefined;
}) {
  if (!replies.length) return null;
  const t = chatTotals(replies);
  // Replies with only a duration (agents that report no tokens) have nothing to total.
  if (!t.tokens && t.cost === null) return null;
  const tilde = t.estimated ? "~" : "";
  const cost =
    t.cost === null ? (
      <Tooltip>
        <TooltipTrigger
          render={
            // Focusable only so keyboard users can open the cost-unknown tooltip; it has no action, so no button role.
            <span tabIndex={0} />
          }
        >
          —
        </TooltipTrigger>
        <TooltipContent>{copy.costUnknown}</TooltipContent>
      </Tooltip>
    ) : (
      <span>
        {tilde}
        {fmtCostPerOp(t.cost)}
      </span>
    );
  return (
    <div
      className={`${stacked ? "flex flex-col gap-0.5" : "items-center gap-2"} text-xs text-muted-foreground tabular-nums ${className ?? ""}`}
      data-testid="chat-totals"
    >
      {partial ? <span>{copy.usageLoaded}</span> : null}
      <span>
        {copy.tokens(`${tilde}${fmtTokens(t.tokens)}`)} · {cost}
      </span>
      {t.withoutUsage ? <span>{copy.turnsWithoutUsage(t.withoutUsage)}</span> : null}
    </div>
  );
}

/** Inline rename (§6.10): Enter or blur saves, Esc cancels, empty reverts, 80-char cap. */
function RenameTitle({
  sessionId,
  title,
  onDone,
}: {
  sessionId: string;
  title: string;
  onDone: () => void;
}) {
  const rename = useRenameChat(sessionId);
  const [value, setValue] = useState(title);
  const [error, setError] = useState<string | null>(null);
  const done = useRef(false);
  const input = useRef<HTMLInputElement>(null);
  // A blur saves only once the input has really held focus: opened from the ⋯ menu, the menu's
  // focus trap can take focus back before it closes, and that must not count as "done".
  const armed = useRef(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => input.current?.focus());
    return () => {
      cancelAnimationFrame(id);
    };
  }, []);
  const save = async () => {
    if (done.current) return;
    done.current = true;
    const next = value.trim();
    if (!next || next === title) {
      onDone();
      return;
    }
    try {
      await rename.mutateAsync(next);
      onDone();
    } catch (err) {
      done.current = false;
      setValue(title);
      const text =
        err instanceof ApiError && err.serverMessage
          ? copy.serverSaid(err.serverMessage)
          : copy.titleEmpty;
      setError(text);
      announce(text);
    }
  };
  return (
    <div className="flex min-w-0 items-center gap-2">
      <Input
        ref={input}
        aria-label={copy.titleLabel}
        value={value}
        maxLength={TITLE_MAX}
        onChange={(e) => {
          setValue(e.target.value);
        }}
        onFocus={() => {
          armed.current = true;
        }}
        onBlur={() => {
          if (armed.current) void save();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void save();
          }
          if (e.key === "Escape") {
            done.current = true;
            onDone();
          }
        }}
        className="h-8 w-64 max-w-full pointer-coarse:h-11"
      />
      {value.length >= TITLE_COUNTER_FROM ? (
        <span className="text-xs text-muted-foreground tabular-nums">
          {copy.titleCounter(value.length, TITLE_MAX)}
        </span>
      ) : null}
      {error ? <span className="text-xs text-destructive">{error}</span> : null}
    </div>
  );
}
