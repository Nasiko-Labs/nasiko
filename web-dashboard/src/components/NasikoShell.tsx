import { ReactNode } from "react";

const LOGO_SRC = "/app/assets/assets/images/nasiko_logo.svg";

type NavKey = "logs" | "metrics";

interface NasikoShellProps {
  children: ReactNode;
  pageTitle: string;
  activeNav: NavKey;
  actions?: ReactNode;
}

export function NasikoShell({
  children,
  pageTitle,
  activeNav,
  actions,
}: NasikoShellProps) {
  return (
    <div className="shell">
      <header className="topbar">
        <div className="topbar-brand">
          <a href="/app/" className="brand-link" title="Back to Nasiko">
            <img
              src={LOGO_SRC}
              alt="Nasiko"
              className="brand-logo"
              width={36}
              height={36}
            />
            <span className="brand-name">Nasiko</span>
          </a>
          <span className="topbar-divider" aria-hidden />
          <nav className="topbar-nav" aria-label="Dashboard sections">
            <a
              href="/logs/"
              className={`nav-link ${activeNav === "logs" ? "active" : ""}`}
            >
              Logs
            </a>
            <a
              href="/metrics/"
              className={`nav-link ${activeNav === "metrics" ? "active" : ""}`}
            >
              Metrics
            </a>
          </nav>
          <span className="topbar-page">{pageTitle}</span>
        </div>
        {actions ? <div className="topbar-actions">{actions}</div> : null}
      </header>
      <div className="shell-content">{children}</div>
    </div>
  );
}
