/**
 * The one page-level loading state: the Nasiko mark drawing itself (our take on Aceternity's LoaderThree,
 * `--animate-mark-draw` in index.css). A page shows it while its primary data loads, in place of section skeletons;
 * the router's pending UI (`RoutePending`) is the same loader. Reduced motion shows the filled mark, still.
 *
 * By default it sits in the middle of the content area (the viewport beside the sidebar, whatever the page header
 * above it), on a fixed layer that lets clicks through. `inline` centres it in its own box instead: a tab panel or a
 * column that loads after the page has shown (pass the box's height in `className`).
 */
import { MARK_BARS } from '@/app/shell/markBars'
import { cn } from '@/lib/utils'

export function PageLoader({
  label = 'Loading',
  inline = false,
  className,
}: {
  /** The image's accessible name, e.g. "Loading agents". */
  label?: string
  inline?: boolean
  className?: string
}) {
  return (
    // Not a live region: the page that loads announces its own state once it has one.
    <div
      aria-busy="true"
      data-testid="page-loader"
      className={cn(
        'flex items-center justify-center',
        inline
          ? 'flex-1'
          : // The sidebar's gap is --sidebar-width, or --sidebar-width-icon in the collapsed rail; none outside the shell.
            'pointer-events-none fixed inset-y-0 right-0 left-0 z-10 md:left-[var(--sidebar-width,0px)] md:group-has-[[data-slot=sidebar][data-collapsible=icon]]/sidebar-wrapper:left-(--sidebar-width-icon)',
        className,
      )}
    >
      <svg
        role="img"
        aria-label={label}
        viewBox="-1 -1 66 66"
        fill="currentColor"
        stroke="currentColor"
        strokeWidth={0.75}
        className="size-14 text-logo"
      >
        {MARK_BARS.map((b, i) => (
          <rect
            key={`${b.x},${b.y}`}
            {...b}
            pathLength={1}
            className="animate-mark-draw [stroke-dasharray:1] motion-reduce:animate-none"
            style={{ animationDelay: `${i * 40}ms` }}
          />
        ))}
      </svg>
    </div>
  )
}
