/**
 * Aceternity UI "Grid Background" (registry item `@aceternity/grid-background-demo`, MIT).
 * Local edits:
 * - copied by hand, like `dot-background.tsx`; the demo's wrapper and headline removed: only the backdrop, filling its
 *   positioned parent.
 * - theme tokens for the hex lines (`--border`) and the white/black fade (`bg-card`, so it sits on a card).
 * - the fade is anchored top-left (where the reading starts) instead of centred.
 * - `aria-hidden` and `pointer-events-none` on the whole layer (decorative only).
 */
import { cn } from '@nasiko/ui/lib/utils'

export function GridBackground({ className }: { className?: string }) {
  return (
    <div aria-hidden className={cn('pointer-events-none absolute inset-0', className)}>
      <div className="absolute inset-0 [background-image:linear-gradient(to_right,var(--border)_1px,transparent_1px),linear-gradient(to_bottom,var(--border)_1px,transparent_1px)] [background-size:32px_32px]" />
      {/* Fades the grid out away from the top-left corner. */}
      <div className="absolute inset-0 bg-card [mask-image:radial-gradient(ellipse_at_top_left,transparent_10%,black_75%)]" />
    </div>
  )
}
