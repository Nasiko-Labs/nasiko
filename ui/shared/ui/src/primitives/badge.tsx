// fidelity: nasiko-ui-lab@1b99bdda: class strings are the lab's `badge` (`[a&]:` link-hover selectors kept); Base UI internals kept.
// Local edits (ported from nasiko-ui-lab@1b99bdda common/src/components/ui/badge.tsx, re-apply after any re-vendor):
// - `link` uses `text-primary-text`, not the bare primary text class (css.test.ts);
// - status variants `success`, `warning`, `info`, `muted` (tinted fill + border + text; pair them with a
//   word or icon, never colour alone); warning's fill is 5%, not 10%: warning text on a 10% tint is 4.4:1 (axe);
//   warning's text stays `text-warning` (the lab's later `text-warning-text` is not taken in stage D: it would recolour);
// - focus is the one app style: a 2 px `--ring` with a 2 px page-coloured offset (DESIGN.md "Focus"), not the 3 px glow.
import { mergeProps } from "@base-ui/react/merge-props"
import { useRender } from "@base-ui/react/use-render"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "../lib/utils"

const badgeVariants = cva(
  "inline-flex w-fit shrink-0 items-center justify-center gap-1 overflow-hidden rounded-full border border-transparent px-2 py-0.5 text-xs font-medium whitespace-nowrap transition-[color,box-shadow] focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&>svg]:pointer-events-none [&>svg]:size-3",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground [a&]:hover:bg-primary/90",
        secondary:
          "bg-secondary text-secondary-foreground [a&]:hover:bg-secondary/90",
        destructive:
          "bg-destructive text-white focus-visible:ring-destructive/20 dark:bg-destructive/60 dark:focus-visible:ring-destructive/40 [a&]:hover:bg-destructive/90",
        outline:
          "border-border text-foreground [a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
        ghost: "[a&]:hover:bg-accent [a&]:hover:text-accent-foreground",
        link: "text-primary-text underline-offset-4 [a&]:hover:underline",
        success: "border-success/40 bg-success/10 text-success",
        warning: "text-warning border-warning/50 bg-warning/5",
        info: "border-info/40 bg-info/10 text-info",
        muted: "border-border bg-muted text-muted-foreground",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function Badge({
  className,
  variant = "default",
  render,
  ...props
}: useRender.ComponentProps<"span"> & VariantProps<typeof badgeVariants>) {
  return useRender({
    defaultTagName: "span",
    props: mergeProps<"span">(
      {
        className: cn(badgeVariants({ variant }), className),
      },
      props
    ),
    render,
    state: {
      slot: "badge",
      variant,
    },
  })
}

export { Badge, badgeVariants }
