/**
 * The village loads with the Overview, not before it: its drawings and motion are their own chunk, so the shell and
 * the page's frame paint first (plans/feat-village-overview.md). Until the chunk arrives, and while the estate loads,
 * the page shows `VillageLoading`: the canvas's own frame at its own height, so nothing below it moves.
 */
import { lazy, Suspense, useLayoutEffect, useRef, useState, type ComponentProps } from 'react'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { copy } from '../copy'
import { fillHeight, FRAME } from './frame'
import { loadVillage } from './load'
import type { VillageCanvas as Canvas } from './VillageCanvas'

const Lazy = lazy(async () => ({ default: (await loadVillage()).VillageCanvas }))

export function VillageCanvas(props: ComponentProps<typeof Canvas>) {
  return (
    <Suspense fallback={<VillageLoading view={props.view} />}>
      <Lazy {...props} />
    </Suspense>
  )
}

/** The canvas's frame with the page loader in it, at the height the canvas will have. */
export function VillageLoading({ view }: { view: 'village' | 'map' }) {
  const box = useRef<HTMLDivElement>(null)
  const [height, setHeight] = useState<number | null>(null)
  useLayoutEffect(() => {
    if (box.current) setHeight(fillHeight(box.current))
  }, [])
  return (
    <div ref={box} className={FRAME} style={height ? { height } : undefined}>
      <PageLoader inline label={copy.views.loading(view)} className="h-full" />
    </div>
  )
}
