// Ported from nasiko-ui-lab@1b99bdda common/src/components/aceternity/dot-background.tsx (Aceternity UI "Dot
// Background", MIT, with the lab's listed edits). Local edits: `cn` from "../lib/cn".
import { cn } from "../lib/cn";

export function DotBackground({ className }: { className?: string }) {
  return (
    <div aria-hidden className={cn("pointer-events-none absolute inset-0", className)}>
      <div className="absolute inset-0 [background-image:radial-gradient(color-mix(in_oklab,var(--muted-foreground)_var(--dot),transparent)_1px,transparent_1px)] [background-size:24px_24px] [--dot:20%] dark:[--dot:12%]" />
      {/* Fades the dots out towards the edges. */}
      <div className="absolute inset-0 bg-background [mask-image:radial-gradient(ellipse_at_center,transparent_20%,black)]" />
    </div>
  );
}
