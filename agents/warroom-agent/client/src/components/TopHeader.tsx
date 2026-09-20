import React from "react";

interface TopHeaderProps {
  companyName: string;
  isScanning: boolean;
  onScan: () => void;
  lastScanTime: string | null;
  searchQuery: string;
  onSearchChange: (q: string) => void;
}

export const TopHeader: React.FC<TopHeaderProps> = ({
  companyName,
  isScanning,
  onScan,
  lastScanTime,
  searchQuery,
  onSearchChange,
}) => {
  return (
    <header className="top-header">
      <div className="header-left">
        <button className="target-select-btn" title="Target Company Context">
          <span>Target: <strong>{companyName}</strong></span>
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <polyline points="6 9 12 15 18 9" />
          </svg>
        </button>

        <div className="status-pill">
          <span className="pulse-dot" />
          <span>Intelligence live</span>
        </div>

        <div className="header-search">
          <svg
            className="search-icon"
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          >
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            type="text"
            placeholder="Search signals, competitors..."
            value={searchQuery}
            onChange={(e) => onSearchChange(e.target.value)}
          />
        </div>
      </div>

      <div className="header-right">
        <div className="last-scan-label">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <circle cx="12" cy="12" r="10" />
            <polyline points="12 6 12 12 16 14" />
          </svg>
          <span>{lastScanTime ? `Last scan: ${lastScanTime}` : "No scan recorded"}</span>
        </div>

        <button
          className="primary-btn"
          onClick={onScan}
          disabled={isScanning}
          id="scan-button"
        >
          {isScanning ? (
            <>
              <span className="spinner" style={{ width: 14, height: 14, borderWidth: 2 }} />
              Scanning...
            </>
          ) : (
            <>
              <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor">
                <polygon points="5 3 19 12 5 21 5 3" />
              </svg>
              Scan Landscape
            </>
          )}
        </button>

        <button className="icon-btn" title="Notifications">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9" />
            <path d="M13.73 21a2 2 0 0 1-3.46 0" />
          </svg>
        </button>
      </div>
    </header>
  );
};
