// Ported from the legacy ui/ on development (3aac84f66) common/src/components/shared/filter-chip.tsx. Local edits:
// imports mapped to ../primitives/toggle; Base UI's Toggle marks its pressed state with `aria-pressed`/`data-pressed`
// (not Radix's `data-state="on"`), so `data-[state=on]:` is `aria-pressed:`; Prettier formatting.
import type { ReactNode } from "react";
import { Toggle } from "../primitives/toggle";

export function FilterChip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <Toggle
      variant="outline"
      size="sm"
      pressed={active}
      onPressedChange={onClick}
      className="min-h-8 rounded-full px-3 text-xs font-normal text-muted-foreground shadow-none hover:bg-transparent hover:text-foreground aria-pressed:border-primary-text aria-pressed:bg-primary/10 aria-pressed:text-primary-text"
    >
      {children}
    </Toggle>
  );
}
