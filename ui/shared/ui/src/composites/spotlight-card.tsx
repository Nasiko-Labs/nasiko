// Ported from the legacy ui/ on development (3aac84f66) common/src/components/aceternity/spotlight-card.tsx
// (8bf09bf29). Local edits: `Card` from ../primitives/card, `cn` from "../lib/cn"; optional props admit `undefined`;
// Prettier formatting.
/**
 * A page's one effect area: a `Card` over Aceternity's grid background (`grid-background.tsx`) with our CSS take on
 * Aceternity's "Card Spotlight" (MIT): a soft `--primary` glow that follows the pointer. Local edits against the
 * original:
 * - no canvas reveal and no JS animation loop: the glow is a radial gradient placed through two CSS variables, set on
 *   pointer move, so a move never re-renders React;
 * - theme tokens only (`--primary` at 9%), so every theme tints it;
 * - decorative: `aria-hidden`, `pointer-events-none`, and hidden under reduced motion (`motion-reduce:hidden`).
 * Used by the TokenOps and Optimization leads; the content goes in `children`, above the effect.
 */
import { useRef, type PointerEvent, type ReactNode } from "react";
import { cn } from "../lib/cn";
import { Card } from "../primitives/card";
import { GridBackground } from "./grid-background";

export function SpotlightCard({
  className,
  children,
}: {
  className?: string | undefined;
  children: ReactNode;
}) {
  const card = useRef<HTMLDivElement>(null);
  const onMove = (e: PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    card.current?.style.setProperty("--spot-x", `${e.clientX - r.left}px`);
    card.current?.style.setProperty("--spot-y", `${e.clientY - r.top}px`);
  };
  return (
    <Card
      ref={card}
      onPointerMove={onMove}
      className={cn("group/spot relative gap-0 overflow-hidden p-0", className)}
    >
      <GridBackground />
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 opacity-0 transition-opacity duration-300 group-hover/spot:opacity-100 motion-reduce:hidden"
        style={{
          background:
            "radial-gradient(420px circle at var(--spot-x, 50%) var(--spot-y, 50%), color-mix(in oklab, var(--primary) 9%, transparent), transparent 70%)",
        }}
      />
      {children}
    </Card>
  );
}
