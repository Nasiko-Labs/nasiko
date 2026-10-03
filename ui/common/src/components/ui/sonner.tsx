/**
 * shadcn's Sonner (`npx shadcn add sonner`). Local edit, re-apply after any `shadcn add sonner`:
 * - no next-themes: Sonner gets the theme already applied to <html> (`.dark`, set by src/app/shell/theme.ts and the
 *   pre-paint script), never "system", so it doesn't read matchMedia itself. Colours follow a later theme switch
 *   anyway, through the `--normal-*` tokens below.
 * - the action button is 44 px on coarse pointers (DESIGN.md touch rule).
 */
import {
  CircleCheckIcon,
  InfoIcon,
  Loader2Icon,
  OctagonXIcon,
  TriangleAlertIcon,
} from "lucide-react"
import { Toaster as Sonner, type ToasterProps } from "sonner"

const Toaster = ({ ...props }: ToasterProps) => {
  const theme = document.documentElement.classList.contains("dark") ? "dark" : "light"

  return (
    <Sonner
      theme={theme}
      className="toaster group"
      toastOptions={{ classNames: { actionButton: "pointer-coarse:min-h-11 pointer-coarse:px-3" } }}
      icons={{
        success: <CircleCheckIcon className="size-4" />,
        info: <InfoIcon className="size-4" />,
        warning: <TriangleAlertIcon className="size-4" />,
        error: <OctagonXIcon className="size-4" />,
        loading: <Loader2Icon className="size-4 animate-spin" />,
      }}
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius)",
        } as React.CSSProperties
      }
      {...props}
    />
  )
}

export { Toaster }
