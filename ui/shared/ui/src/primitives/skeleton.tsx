// fidelity: nasiko-ui-lab@1b99bdda: class strings are the lab's `skeleton` (`bg-accent`); Base UI internals kept.
import { cn } from "../lib/utils"

function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="skeleton"
      className={cn("animate-pulse rounded-md bg-accent", className)}
      {...props}
    />
  )
}

export { Skeleton }
