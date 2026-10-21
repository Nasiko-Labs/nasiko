import React from "react";
import { ScanButton } from "./ScanButton";

interface HeaderProps {
  onScan: () => void;
  isScanning: boolean;
}

export const Header: React.FC<HeaderProps> = ({ onScan, isScanning }) => {
  return (
    <header className="header">
      <div className="header-left">
        <div className="header-title-group">
          <span className="logo-tag">AI Intelligence Ops</span>
          <h1 className="header-title">WARROOM</h1>
          <span className="header-subtitle">Competitive Intelligence & Response</span>
        </div>

        <div className="company-badge">
          <span className="company-label">TARGET:</span>
          <span className="company-name">PAYFLOW</span>
        </div>

        <div className="status-pill">
          <span className="status-dot" />
          <span>LIVE</span>
        </div>
      </div>

      <div className="header-right">
        <ScanButton onScan={onScan} isLoading={isScanning} />
      </div>
    </header>
  );
};
