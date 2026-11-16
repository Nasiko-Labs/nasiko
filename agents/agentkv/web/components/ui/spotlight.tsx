"use client";

import { useCallback, useState, type MouseEvent, type ReactNode } from "react";
import { cn } from "@/lib/report";

export function Spotlight({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  const [spot, setSpot] = useState({ x: 40, y: 30 });

  const onMove = useCallback((event: MouseEvent<HTMLDivElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    setSpot({
      x: ((event.clientX - box.left) / box.width) * 100,
      y: ((event.clientY - box.top) / box.height) * 100,
    });
  }, []);

  return (
    <div
      onMouseMove={onMove}
      className={cn("relative overflow-hidden", className)}
      style={{
        background: `radial-gradient(420px circle at ${spot.x}% ${spot.y}%, rgba(198,255,46,0.16), transparent 55%)`,
      }}
    >
      {children}
    </div>
  );
}
