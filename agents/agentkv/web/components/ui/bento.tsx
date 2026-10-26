import { type ReactNode } from "react";
import { cn } from "@/lib/report";

export function BeamCard({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("relative overflow-hidden rounded-[2px] bg-cream p-px", className)}>
      <div
        aria-hidden
        className="pointer-events-none absolute -inset-[40%] animate-spinSlow motion-reduce:animate-none bg-[conic-gradient(from_90deg,transparent_0_72%,#c6ff2e_78%,transparent_84%)] opacity-80"
      />
      <div className="relative h-full bg-cream">{children}</div>
    </div>
  );
}

export function BentoGrid({ children }: { children: ReactNode }) {
  return (
    <div className="grid gap-px bg-rule md:grid-cols-12 md:grid-rows-[auto_auto]">
      {children}
    </div>
  );
}

export function BentoCell({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return <div className={cn("bg-cream p-7 md:p-8", className)}>{children}</div>;
}
