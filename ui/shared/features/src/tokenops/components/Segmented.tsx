// Ported from the legacy ui/ on development (3aac84f66) common/src/features/tokenops/components/Segmented.tsx
// (8bf09bf29). Local edits: Base UI's `ToggleGroup` (no `type="single"`: a value array, the emptied pick ignored as
// Radix's `v &&` did; the chosen item styled `data-pressed:` for Radix's `data-[state=on]:`); `m` and `transitions`
// from @nasiko/ui/motion; optional props admit `undefined`; Prettier formatting.
/**
 * A single-choice switch (shadcn `ToggleGroup`, Radix radios) whose selection is one pill that glides to the chosen
 * item through a shared `layoutId` (DESIGN.md `morph`), instead of the background snapping across. Under reduced
 * motion `MotionConfig` makes the glide instant. Selection is never colour alone: the chosen item is also `aria-checked`
 * and in the foreground colour.
 */
import { useId } from "react";
import { ToggleGroup, ToggleGroupItem } from "@nasiko/ui/primitives";
import { m, transitions } from "@nasiko/ui/motion";
import { cn } from "@nasiko/ui/cn";

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  className,
}: {
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (v: T) => void;
  label: string;
  className?: string | undefined;
}) {
  const pill = useId();
  return (
    <ToggleGroup
      size="sm"
      spacing={1}
      value={[value]}
      onValueChange={(vs: unknown[]) => {
        const v = vs[0];
        if (typeof v === "string" && v) onChange(v as T);
      }}
      aria-label={label}
      className={cn("rounded-lg border bg-muted/50 p-0.5", className)}
    >
      {options.map((o) => (
        <ToggleGroupItem
          key={o.value}
          value={o.value}
          className="relative h-7 rounded-md px-2.5 text-xs text-muted-foreground hover:bg-transparent hover:text-foreground data-pressed:bg-transparent data-pressed:text-foreground"
        >
          {o.value === value ? (
            <m.span
              layoutId={pill}
              transition={transitions.morph}
              aria-hidden
              className="absolute inset-0 rounded-md border bg-card shadow-xs"
            />
          ) : null}
          <span className="relative">{o.label}</span>
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  );
}
