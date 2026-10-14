// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/components/Outline.tsx. Local edits: optional
// prop typed `| undefined`; Prettier formatting.
/**
 * R2B: a block's place and shape on a server that can't fill it yet, saying what will show there. Static (a dashed
 * outline, no shimmer), so it never reads as loading.
 */
import type { ReactNode } from "react";
import { cn } from "@nasiko/ui/cn";

export function Outline({ children, small }: { children: ReactNode; small?: boolean | undefined }) {
  return (
    <div
      className={cn(
        "flex items-center justify-center rounded-md border border-dashed border-border px-6 text-center text-sm text-muted-foreground",
        small ? "h-24" : "h-55 md:h-60",
      )}
    >
      {children}
    </div>
  );
}
