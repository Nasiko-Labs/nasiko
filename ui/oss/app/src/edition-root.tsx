"use client";

// The OSS edition: the core with no layers (ADR-0007). An OSS build contains no Enterprise module at all; the
// architecture check rejects any import of ui/ee from this tree.
import type { ReactNode } from "react";
import { OSS } from "@nasiko/app-core/edition/edition";
import { EditionProvider } from "@nasiko/app-core/edition/EditionProvider";

export function EditionRoot({ children }: { children: ReactNode }) {
  return <EditionProvider edition={OSS}>{children}</EditionProvider>;
}
