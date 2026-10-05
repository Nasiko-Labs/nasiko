import React from "react";
import { CompetitorResearch, ResearchResult } from "../types";

interface CompetitorCardProps {
  research: CompetitorResearch;
  onViewEvidence: (competitor: string, source: ResearchResult) => void;
}

export const CompetitorCard: React.FC<CompetitorCardProps> = ({ research, onViewEvidence }) => {
  return (
    <article className="competitor-card" aria-label={`Competitor: ${research.competitor}`}>
      <div className="card-header">
        <h3 className="competitor-name">{research.competitor.toUpperCase()}</h3>
        <span className="badge-sources">
          {research.results.length} {research.results.length === 1 ? "SOURCE" : "SOURCES"}
        </span>
      </div>

      <div className="sources-list">
        {research.results.map((item, idx) => (
          <div className="source-item" key={`${item.url}-${idx}`}>
            <div className="source-item-header">
              <h4 className="source-title">{item.title}</h4>
              {(item.date || item.last_updated) && (
                <span className="source-meta-date">{item.date || item.last_updated}</span>
              )}
            </div>

            <p className="source-snippet">{item.snippet}</p>

            <div className="source-actions">
              <button
                className="btn-view-evidence"
                onClick={() => onViewEvidence(research.competitor, item)}
              >
                VIEW EVIDENCE
              </button>
            </div>
          </div>
        ))}

        {research.results.length === 0 && (
          <p style={{ color: "var(--text-muted)", fontSize: "0.85rem", fontStyle: "italic" }}>
            No public evidence returned.
          </p>
        )}
      </div>
    </article>
  );
};
