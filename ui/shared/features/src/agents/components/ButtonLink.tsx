// No lab counterpart: the destination's form of the lab's `<Button asChild><Link …/></Button>` (agents, deploy, mcp
// and the other pages that style an in-app link as a button). Local edits: n/a (new file). Base UI's `Button` would
// add `role="button"` to an anchor, so this is a plain navigation-port `Link` anchor carrying `buttonVariants()` and the
// `data-slot`/`data-variant`/`data-size` attributes the lab's Slot merged onto it (the WaitlistCta pattern).
"use client";

import Link from "@nasiko/app-core/navigation";
import type { ComponentProps } from "react";
import { buttonVariants } from "@nasiko/ui/primitives";

type Variants = NonNullable<Parameters<typeof buttonVariants>[0]>;

export function ButtonLink({
  variant = "default",
  size = "default",
  className,
  ...props
}: ComponentProps<typeof Link> & {
  variant?: Variants["variant"];
  size?: Variants["size"];
}) {
  return (
    <Link
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={buttonVariants({ variant, size, className })}
      {...props}
    />
  );
}
