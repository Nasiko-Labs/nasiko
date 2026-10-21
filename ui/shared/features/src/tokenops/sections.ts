// Ported from the legacy ui/ on development (3aac84f66) common/src/features/tokenops/sections.ts (8bf09bf29). Local
// edits: `prefersReducedMotion` from @nasiko/ui/lib; Prettier formatting.
/** TokenOps' page sections: their DOM ids and the one way to scroll to one (the section nav, `?open=`, the savings strip). */
import { prefersReducedMotion } from "@nasiko/ui/lib";

/** The DOM id of a section's `<section>`. */
export const sectionDomId = (id: string) => `tokenops-section-${id}`;

export function scrollToSection(id: string) {
  const el = document.getElementById(sectionDomId(id));
  if (!el) return false;
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- jsdom has no scrollIntoView (the legacy optional call)
  el.scrollIntoView?.({ block: "start", behavior: prefersReducedMotion() ? "auto" : "smooth" });
  return true;
}
