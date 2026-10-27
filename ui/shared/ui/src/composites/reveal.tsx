/**
 * The page's entrance: tiles and sections rise in once, one `enter.stagger` apart, on the first paint only. Later
 * window or filter changes re-render in place (a dashboard that re-animates on every filter is noise). Under reduced
 * motion `MotionConfig` (app/mount.tsx) drops the rise and keeps the fade.
 */
import { m, type Variants } from 'motion/react'
import type { ComponentProps } from 'react'
import { enter } from '@nasiko/ui/lib/motion'

const group: Variants = { hidden: {}, shown: { transition: { staggerChildren: enter.stagger } } }
const item: Variants = {
  hidden: { opacity: 0, y: enter.rise },
  shown: { opacity: 1, y: 0, transition: enter.transition },
}

/** A stagger group: its `RevealItem` children enter one after another. */
export function Reveal(props: ComponentProps<typeof m.div>) {
  return <m.div variants={group} initial="hidden" animate="shown" {...props} />
}

export function RevealItem(props: ComponentProps<typeof m.div>) {
  return <m.div variants={item} {...props} />
}
