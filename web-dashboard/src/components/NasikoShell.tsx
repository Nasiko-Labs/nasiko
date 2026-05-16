import { ReactNode } from "react";

const LOGO_SRC = "/app/assets/assets/images/nasiko_logo.svg";

interface NasikoShellProps {
  children: ReactNode;
  actions?: ReactNode;
}

export function NasikoShell({ children, actions }: NasikoShellProps) {
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
          <span className="topbar-page">Platform Logs</span>
        </div>
        {actions ? <div className="topbar-actions">{actions}</div> : null}
      </header>
      <div className="shell-content">{children}</div>
    </div>
  );
}
