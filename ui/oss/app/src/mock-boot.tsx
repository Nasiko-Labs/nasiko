"use client";

// The OSS app's mock-mode boot (harness only): the core's MockBoot with no edition handlers (the OSS build is the
// core alone, as the lab's `oss/src/mocks.ts` had none). The shared layout.tsx renders the app's own boot, so the EE
// app can stack its layer's handlers in its `src/mock-boot.tsx` while the route files stay byte-identical.
import type { ReactNode } from "react";
import { MockBoot } from "@nasiko/mocks/mock-boot";

export function EditionMockBoot({ children }: { children: ReactNode }) {
  return <MockBoot>{children}</MockBoot>;
}
