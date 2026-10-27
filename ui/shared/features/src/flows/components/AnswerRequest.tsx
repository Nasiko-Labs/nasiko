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
import { Link } from '@tanstack/react-router'
import { Hand } from 'lucide-react'
import { withButtonStyle } from '@nasiko/ui/primitives/button'
import { rememberRailView } from '@nasiko/features/chat/rememberTarget'
import type { AnswerTarget } from '../answerTargets'
import { copy } from '../copy'

type Size = 'sm' | 'xs'

export function AnswerRequest({ target, size = 'sm' }: { target: AnswerTarget; size?: Size }) {
  const { sessionId, requestId, userId } = target
  return withButtonStyle(
    requestId && sessionId ? (
      <Link to="/chat/$sessionId" params={{ sessionId }} state={{ waitingRequest: requestId }}>
        <Hand className="size-3.5" aria-hidden /> {copy.answer.request}
      </Link>
    ) : (
      <Link to="/chat" onClick={() => userId && rememberRailView(userId, 'waiting')}>
        <Hand className="size-3.5" aria-hidden /> {copy.answer.waiting}
      </Link>
    ),
    { variant: 'outline', size, className: 'w-fit pointer-coarse:min-h-11' },
  )
}
