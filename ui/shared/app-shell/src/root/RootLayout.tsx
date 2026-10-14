import { Suspense, type ComponentType, type ReactNode } from "react";
import { UiProvider } from "@nasiko/ui/theme";
import { MockBoot } from "@nasiko/mocks/mock-boot";

/** Shared browser providers; the static index.html owns the document. */
export function RootApplication({
  children,
  mockBoot: Boot = MockBoot,
}: {
  children: ReactNode;
  mockBoot?: ComponentType<{ children: ReactNode }>;
}) {
  return (
    <UiProvider motion={{ features: "max" }}>
      <Suspense fallback={<p role="status">Loading Nasiko…</p>}>
        <Boot>{children}</Boot>
      </Suspense>
    </UiProvider>
  );
}
