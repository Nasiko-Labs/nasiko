/** Shared class strings for turn views (plan §8.1). */
import { cn } from '@/lib/utils'

/** Targets ≥32 px on fine pointers and ≥44 px on coarse ones (§8.1); `xs` alone is 24 px. */
export const TOUCH = 'min-h-8 pointer-coarse:min-h-11'

export const LINK = cn(
  'inline-flex items-center text-sm text-primary-text underline-offset-4 hover:underline',
  TOUCH,
)

/** `LINK` on a shadcn `<Button variant="link">` (an action that reads as a link, e.g. View trace). */
export const LINK_BUTTON = cn(LINK, 'h-auto p-0 font-normal')

/** The one centred column the header content, every turn and the composer share (v1c §5.5). */
export const CHAT_COLUMN = 'mx-auto w-full max-w-3xl px-4 md:px-6'

/** A lifted panel on the page background (v1c §5.17): the new chat's target list and recent chats. */
export const LIFTED = 'rounded-xl border border-border bg-card shadow-sm'

/**
 * A step chip (live steps and recorded tool calls look the same), on `<Button variant="outline" size="xs">`:
 * these classes undo the button's fixed height, weight and fill.
 */
export const STEP_CHIP = cn(
  'h-auto max-w-full justify-start gap-1 rounded-md border border-border bg-transparent px-2 py-0.5 text-xs font-normal text-muted-foreground shadow-none hover:bg-transparent hover:text-foreground focus-visible:ring-offset-1 has-[>svg]:px-2 dark:border-border dark:bg-transparent dark:hover:bg-transparent',
  TOUCH,
)

/** A `<details>`-style trigger on `<Button variant="ghost" size="xs">` inside `CollapsibleTrigger`: a chevron that turns when open. */
export const DISCLOSE =
  'h-auto justify-start px-0 text-xs font-normal text-muted-foreground hover:bg-transparent hover:text-foreground has-[>svg]:px-0 dark:hover:bg-transparent [&>svg]:transition-transform data-[state=open]:[&>svg]:rotate-90 motion-reduce:[&>svg]:transition-none'

/**
 * A chat rail row (Chats, Waiting): a plain two-line row like the sidebar's own items, no box. Hover is the neutral
 * fill; the open chat takes the theme's tint (`RAIL_ROW_ACTIVE`), with its secondary line in `--accent-foreground`
 * (`--muted-foreground` is under 4.5:1 on Carbon's tint).
 */
export const RAIL_ROW =
  'flex min-h-11 items-center gap-2.5 rounded-md px-2 py-1.5 outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background'
export const RAIL_ROW_ACTIVE = 'bg-accent text-accent-foreground hover:bg-accent'
/** The shared `EmptyState` sized for the 240 px rail (Chats, Waiting, Recorded views). */
export const RAIL_EMPTY = 'mt-1 gap-3 px-3 py-6 md:px-3 md:py-6'
