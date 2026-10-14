"use client";

import Link from "@nasiko/app-core/navigation";
import { usePathname } from "@nasiko/app-core/navigation";
import { useEffect, useState, type ReactNode } from "react";

/**
 * Synthetic nested-navigation fixture (§8.3 global navigation / nested routes).
 * The counter increments once per second while mounted: if a child navigation
 * ever remounted this shell, the counter would reset — the test asserts it
 * does not. The active indicator is a CSS transition on the token duration.
 */
export function NavShell({ children }: { children: ReactNode }) {
  const path = usePathname();
  const [alive, setAlive] = useState(0);
  useEffect(() => {
    const t = setInterval(() => {
      setAlive((a) => a + 1);
    }, 250);
    return () => {
      clearInterval(t);
    };
  }, []);
  const sections = ["usage", "finops", "agents", "broken"];
  return (
    <div className="grid grid-cols-[160px_1fr] gap-4">
      <nav
        aria-label="Sections"
        className="flex flex-col gap-1"
        data-testid="nav"
        data-alive={alive}
      >
        {sections.map((s) => {
          const active = path.endsWith(`/${s}`);
          return (
            <Link
              key={s}
              href={`/harness/motion/nav/${s}`}
              aria-current={active ? "page" : undefined}
              className="relative rounded px-2 py-1 text-sm transition-colors duration-feedback ease-standard hover:bg-muted aria-[current=page]:bg-muted aria-[current=page]:font-medium"
            >
              {s}
            </Link>
          );
        })}
        <span className="mt-2 text-xs text-muted-foreground" data-testid="alive">
          alive {alive}
        </span>
      </nav>
      <div data-testid="child">{children}</div>
    </div>
  );
}
