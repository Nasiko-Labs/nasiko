// Ported from the legacy ui/ on development (3aac84f66) common/src/components/shared/reveal.tsx (8bf09bf29). Local
// edits: `Reveal` is `RevealGroup` here (@nasiko/ui/motion has its own `Reveal` recipe); `m` and `enter` from
// ../motion; Prettier formatting.
/**
 * The page's entrance: tiles and sections rise in once, one `enter.stagger` apart, on the first paint only. Later
 * window or filter changes re-render in place (a dashboard that re-animates on every filter is noise). Under reduced
 * motion the app's `MotionConfig` drops the rise and keeps the fade.
 */
import type { ComponentProps } from "react";
import type { Variants } from "motion/react";
import { enter, m } from "../motion";

const group: Variants = { hidden: {}, shown: { transition: { staggerChildren: enter.stagger } } };
const item: Variants = {
  hidden: { opacity: 0, y: enter.rise },
  shown: { opacity: 1, y: 0, transition: enter.transition },
};

/** A stagger group: its `RevealItem` children enter one after another. */
export function RevealGroup(props: ComponentProps<typeof m.div>) {
  return <m.div variants={group} initial="hidden" animate="shown" {...props} />;
}

export function RevealItem(props: ComponentProps<typeof m.div>) {
  return <m.div variants={item} {...props} />;
}
