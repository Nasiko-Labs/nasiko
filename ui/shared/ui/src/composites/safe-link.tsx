import { ExternalLinkIcon } from "lucide-react";
import type { ComponentPropsWithoutRef } from "react";
import { cn } from "../lib/cn";
import { isSafeHref, type HrefPolicy } from "@nasiko/contracts/content-policy";

/**
 * The only way data-supplied links reach the DOM. Validates with the shared
 * content policy; an unsafe href renders the label as plain text (never a
 * dead or dangerous anchor). External links get `rel="noopener noreferrer"`
 * and a visible marker; same-origin links open in place.
 */
export interface SafeLinkProps extends Omit<
  ComponentPropsWithoutRef<"a">,
  "href" | "rel" | "target"
> {
  readonly href: unknown;
  readonly policy?: HrefPolicy;
  readonly external?: "auto" | "always" | "never";
}

export function SafeLink({
  href,
  policy,
  external = "auto",
  className,
  children,
  ...rest
}: SafeLinkProps) {
  if (!isSafeHref(href, policy)) {
    return (
      <span className={cn("text-muted-foreground", className)} data-unsafe-link="">
        {children}
      </span>
    );
  }
  const abs = /^[a-z][a-z0-9+.-]*:/i.test(href);
  const isExternal = external === "always" || (external === "auto" && abs);
  return (
    <a
      href={href}
      className={cn(
        "inline-flex items-center gap-1 text-primary underline-offset-4 hover:underline",
        className,
      )}
      {...(isExternal ? { target: "_blank", rel: "noopener noreferrer" } : {})}
      {...rest}
    >
      {children}
      {isExternal ? (
        <ExternalLinkIcon className="size-3" aria-label="(opens in a new tab)" />
      ) : null}
    </a>
  );
}
