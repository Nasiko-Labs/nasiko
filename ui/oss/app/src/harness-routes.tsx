/** Imported only when the build admits synthetic fixtures. Never present in a release bundle. */
import { Component, type ReactNode } from "react";
import type { AppRoute } from "@nasiko/app-shell/router";
import { useParams } from "@nasiko/app-core/navigation";
import FoundationLayout from "./app/(foundation)/layout";
import { MotionHarness } from "./components/harness/motion-harness";
import { NavShell } from "./components/harness/nav-shell";
import SectionPage from "./app/(foundation)/harness/motion/nav/[section]/page.harness";

class SectionBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  override state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  override render() {
    return this.state.failed ? (
      <div role="alert" data-testid="section-error">
        This section failed to load.
        <button
          onClick={() => {
            this.setState({ failed: false });
          }}
        >
          Retry
        </button>
      </div>
    ) : (
      this.props.children
    );
  }
}
function NavFixture() {
  const { section } = useParams();
  return (
    <NavShell>
      <SectionBoundary key={String(section)}>
        <SectionPage />
      </SectionBoundary>
    </NavShell>
  );
}
const pages = import.meta.glob<{ default: React.ComponentType }>("./app/**/page.harness.tsx");
export const HARNESS_ROUTES: AppRoute[] = Object.entries(pages).map(([file, load]) => {
  const path = file.replace("./app/(foundation)", "").replace("/page.harness.tsx", "");
  return {
    path,
    public: true,
    load: async () => {
      const { default: Page } = await load();
      return {
        default: () => (
          <FoundationLayout>
            {path.includes("/motion") ? (
              <MotionHarness>{path.includes("/nav/") ? <NavFixture /> : <Page />}</MotionHarness>
            ) : (
              <Page />
            )}
          </FoundationLayout>
        ),
      };
    },
  };
});
