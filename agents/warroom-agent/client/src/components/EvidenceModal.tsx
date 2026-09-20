import React, { useEffect } from "react";
import { ResearchResult } from "../types";

interface EvidenceModalProps {
  competitor: string;
  source: ResearchResult | null;
  onClose: () => void;
}

export const EvidenceModal: React.FC<EvidenceModalProps> = ({ competitor, source, onClose }) => {
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  if (!source) return null;

  return (
    <div className="modal-overlay" onClick={onClose} role="dialog" aria-modal="true">
      <div className="modal-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <div className="modal-title-group">
            <span className="modal-tag">{competitor} EVIDENCE</span>
            <h2 className="modal-title">Source Verification</h2>
          </div>
          <button className="btn-close" onClick={onClose} aria-label="Close dialog">
            ✕
          </button>
        </div>

        <div className="modal-body">
          <div className="evidence-field">
            <span className="evidence-label">DOCUMENT TITLE</span>
            <p style={{ color: "var(--text-primary)", fontWeight: 600 }}>{source.title}</p>
          </div>

          <div className="evidence-field">
            <span className="evidence-label">SOURCE URL</span>
            <a
              href={source.url}
              target="_blank"
              rel="noopener noreferrer"
              style={{
                color: "var(--accent-cyan)",
                fontFamily: "var(--font-mono)",
                fontSize: "0.8rem",
                wordBreak: "break-all",
              }}
            >
              {source.url}
            </a>
          </div>

          <div className="evidence-field">
            <span className="evidence-label">VERIFIED SNIPPET</span>
            <div className="evidence-snippet-box">{source.snippet}</div>
          </div>

          <div className="evidence-meta-row">
            <div className="evidence-field">
              <span className="evidence-label">PUBLICATION DATE</span>
              <p style={{ fontFamily: "var(--font-mono)", fontSize: "0.8rem", color: "var(--text-secondary)" }}>
                {source.date || "Not reported in source"}
              </p>
            </div>

            <div className="evidence-field">
              <span className="evidence-label">LAST UPDATED</span>
              <p style={{ fontFamily: "var(--font-mono)", fontSize: "0.8rem", color: "var(--text-secondary)" }}>
                {source.last_updated || "Not reported in source"}
              </p>
            </div>
          </div>
        </div>

        <div className="modal-footer">
          <button className="btn-secondary" onClick={onClose}>
            Close
          </button>
          <a
            className="btn-open-source"
            href={source.url}
            target="_blank"
            rel="noopener noreferrer"
          >
            <span>OPEN SOURCE</span>
            <span>↗</span>
          </a>
        </div>
      </div>
    </div>
  );
};
