"use client";

// Ported from nasiko-ui-lab@1b99bdda common/src/components/ui/sonner.tsx (shadcn Sonner with the lab's edits: no
// next-themes, the theme already applied to <html>; a 44 px action button on coarse pointers). Local edits: the
// page is server-rendered here, so the theme is read from <html> after hydration through
// `useSyncExternalStore` (the server snapshot is "light"; the toaster renders nothing visible until a toast
// exists) and follows later theme switches; `toast` is re-exported so features have one owner for toasts
// ("@nasiko/ui/toast") instead of importing sonner directly.
import {
  CircleCheckIcon,
  InfoIcon,
  Loader2Icon,
  OctagonXIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { useSyncExternalStore, type CSSProperties } from "react";
import { Toaster as Sonner, toast, type ToasterProps } from "sonner";

function subscribe(onChange: () => void): () => void {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  return () => {
    observer.disconnect();
  };
}
const themeNow = () => (document.documentElement.classList.contains("dark") ? "dark" : "light");

function Toaster({ ...props }: ToasterProps) {
  const theme = useSyncExternalStore(subscribe, themeNow, () => "light" as const);
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
        } as CSSProperties
      }
      {...props}
    />
  );
}

export { Toaster, toast };
export type { ExternalToast } from "sonner";
