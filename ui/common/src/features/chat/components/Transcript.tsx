/**
 * The one scroller (plan §6.9): follows new output while within FOLLOW_PX of the bottom, else
 * offers Jump to latest; Load older keeps the first visible turn where it was.
 */
import { ArrowDown, RotateCw } from 'lucide-react'
import { useCallback, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import { tuning } from '../tuning'
import { CHAT_COLUMN } from './turnStyles'

export function Transcript({
  children,
  contentKey,
  hasOlder,
  loadingOlder,
  olderFailed,
  onLoadOlder,
}: {
  children: ReactNode
  /** Changes whenever the rendered content grows (turn count + live text length). */
  contentKey: string
  hasOlder: boolean
  loadingOlder: boolean
  olderFailed: boolean
  onLoadOlder(): void
}) {
  const scroller = useRef<HTMLDivElement>(null)
  const following = useRef(true)
  const anchor = useRef<{ height: number; top: number } | null>(null)
  const [atBottom, setAtBottom] = useState(true)

  const onScroll = useCallback(() => {
    const el = scroller.current
    if (!el) return
    const near = el.scrollHeight - el.scrollTop - el.clientHeight <= tuning.FOLLOW_PX
    following.current = near
    setAtBottom(near)
  }, [])

  useLayoutEffect(() => {
    const el = scroller.current
    if (!el) return
    if (anchor.current) {
      // Older rows were prepended: keep the same content under the viewport.
      el.scrollTop = anchor.current.top + (el.scrollHeight - anchor.current.height)
      anchor.current = null
      return
    }
    if (following.current) el.scrollTop = el.scrollHeight
  }, [contentKey])

  const loadOlder = () => {
    const el = scroller.current
    if (el) anchor.current = { height: el.scrollHeight, top: el.scrollTop }
    onLoadOlder()
  }

  const jump = () => {
    const el = scroller.current
    if (!el) return
    following.current = true
    setAtBottom(true)
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' })
  }

  return (
    <div className="relative min-h-0 flex-1">
      <div
        ref={scroller}
        id="chat-transcript"
        onScroll={onScroll}
        tabIndex={-1}
        className="h-full overflow-y-auto outline-none"
        data-testid="transcript"
      >
        <div className={cn(CHAT_COLUMN, 'space-y-6 py-6')}>
          {olderFailed ? (
            <div className="flex items-center justify-center gap-2 text-sm text-muted-foreground">
              {copy.olderFailed}{' '}
              <Button
                size="xs"
                variant="outline"
                className="min-h-8 pointer-coarse:min-h-11"
                onClick={loadOlder}
              >
                <RotateCw aria-hidden /> {copy.retry}
              </Button>
            </div>
          ) : hasOlder ? (
            <div className="flex justify-center">
              <Button
                size="sm"
                variant="ghost"
                className="pointer-coarse:min-h-11"
                disabled={loadingOlder}
                onClick={loadOlder}
              >
                {loadingOlder ? copy.loading : copy.loadOlder}
              </Button>
            </div>
          ) : null}
          {children}
        </div>
      </div>
      {!atBottom ? (
        <Button
          size="sm"
          variant="secondary"
          className="absolute bottom-3 left-1/2 -translate-x-1/2 shadow pointer-coarse:min-h-11"
          onClick={jump}
        >
          <ArrowDown aria-hidden /> {copy.jumpToLatest}
        </Button>
      ) : null}
    </div>
  )
}
