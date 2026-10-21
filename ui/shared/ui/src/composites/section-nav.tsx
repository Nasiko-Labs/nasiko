// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/section-nav.tsx.
// Router-independent href/component port; anchors share the admitted button variant classes.
// Navigation retains native link semantics, rather than rendering a Button with role="button". Legacy ui/ on
// development (3aac84f66, c721a11a8): the sections are underlined tabs (the top-bar level is the app-core binding's).
/**
 * A module's sub-pages as one row of links under its page header (Workflows: Deployed · Drafts · Runs; Agents: All
 * agents · Your agents · Builds), so a module never needs a second sidebar.
 */
import type { ComponentType, ReactNode } from "react";
import { cn } from "../lib/cn";

export interface Section {
  to: string;
  label: string;
}

/** What the section anchor renders with: an `href` plus the attributes the nav sets. */
export type SectionLinkProps = {
  href: string;
  className?: string | undefined;
  "aria-current"?: "page" | undefined;
  children?: ReactNode;
};

export function SectionNav({
  label,
  sections,
  current,
  link,
}: {
  /** The landmark's name (the module). */
  label: string;
  sections: readonly Section[];
  current: Section["to"];
  /** The anchor component (a framework `Link`); a plain `<a>` by default. */
  link?: ComponentType<SectionLinkProps> | "a" | undefined;
}) {
  const Link = link ?? "a";
  return (
    // Underlined, full width: the module's sections read as the page's tabs, not as loose buttons.
    <nav aria-label={label} className="flex gap-1 overflow-x-auto border-b border-border">
      {sections.map((s) => (
        <Link
          key={s.to}
          href={s.to}
          aria-current={s.to === current ? "page" : undefined}
          className={cn(
            "-mb-px inline-flex min-h-10 shrink-0 items-center border-b-2 px-3 text-sm transition-colors focus-visible:outline-2 focus-visible:outline-ring motion-reduce:transition-none",
            s.to === current
              ? "border-foreground font-medium text-foreground"
              : "border-transparent text-muted-foreground hover:text-foreground",
          )}
        >
          {s.label}
        </Link>
      ))}
    </nav>
  );
}
