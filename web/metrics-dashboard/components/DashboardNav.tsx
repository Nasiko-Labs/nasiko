"use client";

export type DashboardView = "fleet" | "agent";

type Props = {
  view: DashboardView;
  onChange: (view: DashboardView) => void;
};

export function DashboardNav({ view, onChange }: Props) {
  return (
    <nav className="dashboard-nav" aria-label="Dashboard views">
      <button
        type="button"
        className={`dashboard-nav__btn${view === "fleet" ? " dashboard-nav__btn--active" : ""}`}
        aria-current={view === "fleet" ? "page" : undefined}
        onClick={() => onChange("fleet")}
      >
        All agents
      </button>
      <button
        type="button"
        className={`dashboard-nav__btn${view === "agent" ? " dashboard-nav__btn--active" : ""}`}
        aria-current={view === "agent" ? "page" : undefined}
        onClick={() => onChange("agent")}
      >
        Single agent
      </button>
    </nav>
  );
}
