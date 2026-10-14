// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/components/BuildBadge.tsx. Local edits: none beyond
// imports and Prettier formatting.
"use client";

import { Badge } from "@nasiko/ui/primitives";
import { BUILD_BADGE, type BadgeKey } from "../steps";

/** A build's status badge: status tokens only, never the accent, and always a word (design review 12). */
export function BuildBadge({ badge }: { badge: BadgeKey }) {
  const b = BUILD_BADGE[badge];
  return (
    <Badge variant={b.variant} data-status={badge}>
      {b.label}
    </Badge>
  );
}
