import React from "react";

interface ScanButtonProps {
  onScan: () => void;
  isLoading: boolean;
  disabled?: boolean;
}

export const ScanButton: React.FC<ScanButtonProps> = ({ onScan, isLoading, disabled = false }) => {
  return (
    <button
      className="btn-scan"
      onClick={onScan}
      disabled={isLoading || disabled}
      aria-label="Scan Competitive Landscape"
    >
      {isLoading ? (
        <>
          <div className="spinner" />
          <span>SCANNING LANDSCAPE...</span>
        </>
      ) : (
        <>
          <span>⚡</span>
          <span>SCAN LANDSCAPE</span>
        </>
      )}
    </button>
  );
};
