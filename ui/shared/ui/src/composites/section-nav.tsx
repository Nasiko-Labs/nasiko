// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/section-nav.tsx.
// Router-independent href/component port; anchors share the admitted button variant classes.
// Navigation retains native link semantics, rather than rendering a Button with role="button".
/**
 * A module's sub-pages as one row of links under its page header (Workflows: Deployed · Drafts · Runs; Agents: All
 * agents · Your agents · Builds), so a module never needs a second sidebar.
 */
import type { ComponentType, ReactNode } from "react";
import { buttonVariants } from "../primitives/button";

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
    <nav aria-label={label} className="flex flex-wrap gap-1">
      {sections.map((s) => (
        <Link
          key={s.to}
          href={s.to}
          aria-current={s.to === current ? "page" : undefined}
          className={buttonVariants({
            size: "sm",
            variant: s.to === current ? "secondary" : "ghost",
            className: s.to === current ? "font-medium" : "font-normal text-muted-foreground",
          })}
        >
          {s.label}
        </Link>
      ))}
    </nav>
  );
}
