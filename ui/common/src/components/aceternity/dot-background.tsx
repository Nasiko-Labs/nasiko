/**
 * Aceternity UI "Dot Background" (registry item `@aceternity/dot-background-demo`, MIT).
 * Local edits:
 * - copied by hand: the registry item lists `mini-svg-data-uri`, which the component never uses.
 * - the demo's wrapper and "Backgrounds" headline removed; this is only the backdrop, filling its positioned parent.
 * - theme tokens for the hex dots (`--muted-foreground` at 20% light, 12% dark) and the white/black fade (`bg-background`).
 * - `aria-hidden` and `pointer-events-none` on the whole layer (decorative only).
 */
import { cn } from '@/lib/utils'

export function DotBackground({ className }: { className?: string }) {
  return (
    <div aria-hidden className={cn('pointer-events-none absolute inset-0', className)}>
      <div className="absolute inset-0 [background-image:radial-gradient(color-mix(in_oklab,var(--muted-foreground)_var(--dot),transparent)_1px,transparent_1px)] [background-size:24px_24px] [--dot:20%] dark:[--dot:12%]" />
      {/* Fades the dots out towards the edges. */}
      <div className="absolute inset-0 bg-background [mask-image:radial-gradient(ellipse_at_center,transparent_20%,black)]" />
    </div>
  )
}
