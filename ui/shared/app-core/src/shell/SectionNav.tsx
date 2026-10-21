// Ported from the legacy ui/ on development (3aac84f66) common/src/components/shared/section-nav.tsx (e8379db5a: the
// open section's level in the top bar). Local edits: the row itself stays `@nasiko/ui`'s `SectionNav` (which can't
// reach the shell's top bar); this binding adds the level through `TopBar` and links with the navigation port's `Link`.
"use client";

import { SectionNav as SectionRow, type Section } from "@nasiko/ui/composites";
import Link from "../navigation";
import { TopBar } from "./TopBar";

export type { Section } from "@nasiko/ui/composites";

/**
 * A module's sub-pages as one row of links under its page header, with the open one's level in the top bar: the
 * module's first section is its nav item ("Agents"); the others are a level below it ("Agents › Builds").
 */
export function SectionNav({
  label,
  sections,
  current,
}: {
  label: string;
  sections: readonly Section[];
  current: Section["to"];
}) {
  const at = sections.find((s) => s.to === current);
  return (
    <>
      <TopBar trail={at && at !== sections[0] ? [at.label] : []} />
      <SectionRow label={label} sections={sections} current={current} link={Link} />
    </>
  );
}
