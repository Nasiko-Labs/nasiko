// Ported from nasiko-ui-lab@1b99bdda common/src/features/flows/components/AnswerRequest.tsx. Local edits: `<Button
// asChild><Link>` is a `ButtonLink` (../../agents/components/ButtonLink.tsx) per branch; the matched request's link is
// `/chat/<encoded id>` and its click hands the request to the chat through chat/waitingNav.ts `openFromWaiting` (the
// lab's `<Link state={{ waitingRequest }}>`: the navigation port's links carry no state); `size` admits `undefined`
// (exactOptionalPropertyTypes); Prettier formatting.
/**
 * "Answer the request" for a call waiting on a human (plans/feat-flows.md F20, eng review O2). A request carries its
 * chat (`execution.context_id` / `chat_session_id`) and agent but no flow id, so the link opens the pending request
 * whose chat and agent both match exactly one (`answer.ts`); otherwise (no session, no match, two matches, or the
 * requests can't be read) it opens chat's Waiting view, which lists every request.
 *
 * The page reads the pending requests once (`answerTargets.ts` `useAnswerTargets`, only while a call waits and the signed-in user is
 * known: their poll and backoff are per user) and hands each button its target, so several waiting calls never
 * start several polls.
 */
import { Hand } from "lucide-react";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { rememberRailView } from "../../chat/rememberTarget";
import { openFromWaiting } from "../../chat/waitingNav";
import type { AnswerTarget } from "../answerTargets";
import { copy } from "../copy";

type Size = "sm" | "xs";

export function AnswerRequest({
  target,
  size = "sm",
}: {
  target: AnswerTarget;
  size?: Size | undefined;
}) {
  const { sessionId, requestId, userId } = target;
  const className = "w-fit pointer-coarse:min-h-11";
  return requestId && sessionId ? (
    <ButtonLink
      href={`/chat/${encodeURIComponent(sessionId)}`}
      onClick={() => {
        openFromWaiting(sessionId, requestId);
      }}
      size={size}
      variant="outline"
      className={className}
    >
      <Hand className="size-3.5" aria-hidden /> {copy.answer.request}
    </ButtonLink>
  ) : (
    <ButtonLink
      href="/chat"
      onClick={() => {
        if (userId) rememberRailView(userId, "waiting");
      }}
      size={size}
      variant="outline"
      className={className}
    >
      <Hand className="size-3.5" aria-hidden /> {copy.answer.waiting}
    </ButtonLink>
  );
}
