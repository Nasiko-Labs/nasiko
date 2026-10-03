/**
 * Presenter mode (`?demo=1`, approved at the final gate as X1): a small step card for the
 * three-click "follow the money" path. The step comes from the URL, so it always matches
 * the screen; the card never blocks content and can be dismissed.
 */
import { useNavigate, useRouterState } from '@tanstack/react-router'
import { X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { presenterStep } from './context'

const STEPS = [
  {
    title: 'Find the spike',
    body: 'The summary names the most expensive day. Click "See sessions" next to it.',
  },
  {
    title: 'Pick the costliest session',
    body: "That day's sessions, most expensive first. Open the top one.",
  },
  {
    title: 'Read why it cost what it did',
    body: 'The narrative names the cause; the failing span is already selected.',
  },
] as const

export function PresenterCard() {
  const location = useRouterState({ select: (s) => s.location })
  const navigate = useNavigate()
  const search = location.search as Record<string, unknown>
  // TanStack Router JSON-parses search values: `?demo=1` arrives as the number 1.
  if (!([true, 1, 'true', '1'] as unknown[]).includes(search.demo)) return null
  const step = presenterStep(location.pathname, search)
  if (step === null) return null
  const s = STEPS[step]
  return (
    <>
      {/* Room below the page so the fixed card never hides the last rows (it sits over them otherwise). */}
      <div aria-hidden className="h-44" />
      <Card
        asChild
        className="fixed inset-x-4 bottom-4 z-40 gap-0 rounded-lg p-4 text-sm sm:left-auto sm:w-72"
      >
        <aside aria-label="Demo steps">
          <div className="flex items-start justify-between gap-2">
            <p className="text-xs font-medium text-muted-foreground">
              Step {step + 1} of {STEPS.length}
            </p>
            <Button
              variant="ghost"
              size="icon"
              className="-mt-2 -mr-2 size-8"
              aria-label="Close demo steps"
              onClick={() =>
                void navigate({
                  to: '.',
                  search: (prev: Record<string, unknown>) => ({ ...prev, demo: undefined }),
                  replace: true,
                })
              }
            >
              <X className="size-4" aria-hidden />
            </Button>
          </div>
          <p className="font-medium">{s.title}</p>
          <p className="mt-1 text-muted-foreground">{s.body}</p>
          <ol className="mt-3 flex gap-1" aria-hidden>
            {STEPS.map((dot, i) => (
              <li
                key={dot.title}
                className={
                  i <= step
                    ? 'h-1 flex-1 rounded-full bg-primary'
                    : 'h-1 flex-1 rounded-full bg-muted'
                }
              />
            ))}
          </ol>
        </aside>
      </Card>
    </>
  )
}
