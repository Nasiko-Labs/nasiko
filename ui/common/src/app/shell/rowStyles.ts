/**
 * Sidebar row classes (design review 7A). Kept out of the component files so fast refresh keeps
 * working (component files export only components).
 */

/**
 * Every row: 32 px (44 px on touch, in the rail too: the primitive's `size-8!` would win otherwise),
 * 6 px radius, a 1 px border slot so the active row doesn't shift, 2 px focus ring.
 */
export const ROW =
  'h-8 pointer-coarse:h-11 pointer-coarse:group-data-[collapsible=icon]:size-11! rounded-md border border-transparent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-sidebar'

/** Side padding of the header, groups and footer: 12 px, 6 px in the touch rail so a 44 px row fits in 56 px. */
export const GROUP = 'px-3 pointer-coarse:group-data-[collapsible=icon]:px-1.5'

/**
 * The current page's item: the theme's pale `--accent` tint with `--accent-foreground` text (§6.4). Hover
 * on other rows is neutral (`--sidebar-accent` maps to `--muted`), so it never reads as selected.
 */
export const ACTIVE_ROW =
  'data-[active=true]:bg-accent data-[active=true]:font-medium data-[active=true]:text-accent-foreground data-[active=true]:hover:bg-accent data-[active=true]:hover:text-accent-foreground'

/** Labels fade as the sidebar narrows (the width transition itself lives in sidebar.tsx). */
export const LABEL =
  'transition-opacity duration-[180ms] ease-out motion-reduce:transition-none group-data-[collapsible=icon]:opacity-0'
