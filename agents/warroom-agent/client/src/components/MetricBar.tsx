import React from "react";

interface MetricBarProps {
  competitorsCount: number;
  sourcesCount: number;
  signalsCount: number;
  highImpactCount: number;
}

export const MetricBar: React.FC<MetricBarProps> = ({
  competitorsCount,
  sourcesCount,
  signalsCount,
  highImpactCount,
}) => {
  return (
    <section className="metric-bar" aria-label="System Metrics">
      <div className="metric-card">
        <span className="metric-label">COMPETITORS</span>
        <span className="metric-value">{competitorsCount}</span>
        <span className="metric-hint">Monitored peers</span>
      </div>

      <div className="metric-card">
        <span className="metric-label">SOURCES</span>
        <span className="metric-value">{sourcesCount}</span>
        <span className="metric-hint">Verified web evidence</span>
      </div>

      <div className="metric-card">
        <span className="metric-label">SIGNALS</span>
        <span className="metric-value">{signalsCount}</span>
        <span className="metric-hint">AI-derived changes</span>
      </div>

      <div className="metric-card">
        <span className="metric-label">HIGH IMPACT</span>
        <span className="metric-value">{highImpactCount}</span>
        <span className="metric-hint">Requires strategic response</span>
      </div>
    </section>
  );
};
