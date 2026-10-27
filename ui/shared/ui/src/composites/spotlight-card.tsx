/**
 * A page's one effect area (CLAUDE.md "Aceternity effects"): a shadcn `Card` over Aceternity's grid background
 * (`grid-background.tsx`) with our CSS take on Aceternity's "Card Spotlight" (MIT): a soft `--primary` glow that follows
 * the pointer. Local edits against the original:
 * - no canvas reveal and no JS animation loop: the glow is a radial gradient placed through two CSS variables, set on
 *   pointer move, so a move never re-renders React;
 * - theme tokens only (`--primary` at 9%), so every theme tints it;
 * - decorative: `aria-hidden`, `pointer-events-none`, and hidden under reduced motion (`motion-reduce:hidden`).
 * Used by the TokenOps and Optimization leads; the content goes in `children`, above the effect.
 */
import { useRef, type PointerEvent, type ReactNode } from 'react'
import { GridBackground } from '@nasiko/ui/composites/grid-background'
import { Card } from '@nasiko/ui/primitives/card'
import { cn } from '@nasiko/ui/lib/utils'

export function SpotlightCard({
  className,
  children,
}: {
  className?: string
  children: ReactNode
}) {
  const card = useRef<HTMLDivElement>(null)
  const onMove = (e: PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect()
    card.current?.style.setProperty('--spot-x', `${e.clientX - r.left}px`)
    card.current?.style.setProperty('--spot-y', `${e.clientY - r.top}px`)
  }
  return (
    <Card
      ref={card}
      onPointerMove={onMove}
      className={cn('group/spot relative gap-0 overflow-hidden p-0', className)}
    >
      <GridBackground />
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 opacity-0 transition-opacity duration-300 group-hover/spot:opacity-100 motion-reduce:hidden"
        style={{
          background:
            'radial-gradient(420px circle at var(--spot-x, 50%) var(--spot-y, 50%), color-mix(in oklab, var(--primary) 9%, transparent), transparent 70%)',
        }}
      />
      {children}
    </Card>
  )
}
