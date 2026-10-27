// fidelity: nasiko-ui-lab@1b99bdda: class strings are the lab's `button` (Radix data-state → Base UI equivalents where they differ, `data-variant`/`data-size` attributes as the lab sets them); Base UI internals kept.
// destination: `buttonVariants()` returns `cn`-merged classes (links styled as buttons matched the lab only through `Button`).
// Local edits (ported from nasiko-ui-lab@1b99bdda common/src/components/ui/button.tsx, re-apply after any re-vendor):
// - focus is a 2 px `--ring` with a 2 px page-coloured offset (DESIGN.md "Focus"), not the 3 px glow;
// - the transition names colours and opacity only (`transition-all` animated layout on resize);
// - `destructive` drops the upstream focus-ring recolour, so every button focuses the same way;
// - `link` uses `text-primary-text`, not the bare primary text class (css.test.ts);
// - the default variant hovers to `--primary-hover` (a designed shade per theme), not `primary/80`.
import * as React from "react"
import { Button as ButtonPrimitive } from "@base-ui/react/button"
import { cva, type VariantProps } from "class-variance-authority"
import { cn } from "../lib/utils"

const buttonVariantClasses = cva(
  "inline-flex shrink-0 items-center justify-center gap-2 rounded-md text-sm font-medium whitespace-nowrap transition-[color,background-color,border-color,opacity] outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary-hover",
        destructive:
          "bg-destructive text-white hover:bg-destructive/90 dark:bg-destructive/60",
        outline:
          "border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground dark:border-input dark:bg-input/30 dark:hover:bg-input/50",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        ghost:
          "hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50",
        link: "text-primary-text underline-offset-4 hover:underline",
      },
      size: {
        default: "h-9 px-4 py-2 has-[>svg]:px-3",
        xs: "h-6 gap-1 rounded-md px-2 text-xs has-[>svg]:px-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-8 gap-1.5 rounded-md px-3 has-[>svg]:px-2.5",
        lg: "h-10 rounded-md px-6 has-[>svg]:px-4",
        icon: "size-9",
        "icon-xs": "size-6 rounded-md [&_svg:not([class*='size-'])]:size-3",
        "icon-sm": "size-8",
        "icon-lg": "size-10",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  ...props
}: ButtonPrimitive.Props & VariantProps<typeof buttonVariantClasses>) {
  return (
    <ButtonPrimitive
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={buttonVariants({ variant, size, className })}
      {...props}
    />
  )
}

/**
 * Destination: the variant classes merged through `cn`, so a size's `gap-1.5` replaces the base `gap-2` wherever the
 * classes are used, not only inside `Button` (links styled as buttons call this directly; the lab's `Button asChild`
 * merged through `cn` the same way). Without the merge both classes were emitted and the base `gap-2` won.
 */
function buttonVariants(
  props?: Parameters<typeof buttonVariantClasses>[0]
): string {
  return cn(buttonVariantClasses(props))
}

/**
 * What an element styled as a button carries: the lab's `<Button asChild><Link …/></Button>`, where Radix's Slot merged
 * the button's classes and `data-slot`/`data-variant`/`data-size` onto the child. Base UI's `Button` would add
 * `role="button"` to an anchor, so a link stays a link and spreads this instead (codex's ButtonLink, as one helper).
 */
function asButton({
  variant = "default",
  size = "default",
  className,
}: VariantProps<typeof buttonVariantClasses> & { className?: string }) {
  return {
    "data-slot": "button",
    "data-variant": variant ?? "default",
    "data-size": size ?? "default",
    className: buttonVariants({ variant, size, className }),
  }
}

/**
 * `asButton` for a child known only when it renders (a prop, a variable, one of several links): clones it as Radix's
 * Slot did, the child's own props winning and the classes merged.
 */
function withButtonStyle(
  child: React.ReactNode,
  options: Parameters<typeof asButton>[0]
) {
  if (!React.isValidElement<{ className?: string }>(child)) return child
  const styled = asButton(options)
  return React.cloneElement(child, {
    ...styled,
    ...child.props,
    className: cn(styled.className, child.props.className),
  })
}

export { Button, asButton, buttonVariants, withButtonStyle }
