"use client";

import { useMemo } from "react";
import {
  Background,
  Controls,
  ReactFlow,
  type Edge,
  type Node,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { formatLatencyMs } from "@/lib/format";
import type { TraceDetailResponse } from "@/lib/types";
import { workflowNodeTypes } from "@/components/workflow/WorkflowNodes";

type Props = {
  trace: TraceDetailResponse | null;
  loading?: boolean;
  error?: string | null;
  sessionId?: string | null;
  fullscreen?: boolean;
  onToggleFullscreen?: () => void;
  compact?: boolean;
};

export function TraceGraphPanel({
  trace,
  loading,
  error,
  sessionId,
  fullscreen = false,
  onToggleFullscreen,
  compact = false,
}: Props) {
  const nodes: Node[] = useMemo(() => {
    if (!trace) return [];
    return trace.nodes.map((n) => ({
      id: n.id,
      position: n.position,
      data: n.data,
      type: "workflow",
    }));
  }, [trace]);

  const edges: Edge[] = useMemo(() => {
    if (!trace) return [];
    return trace.edges.map((e) => ({
      id: e.id,
      source: e.source,
      target: e.target,
      type: "smoothstep",
      animated: true,
      style: { stroke: "var(--border-strong, #64748b)", strokeWidth: 2 },
    }));
  }, [trace]);

  const canvasClass = [
    "trace-canvas",
    compact ? "trace-canvas--compact" : "",
    fullscreen ? "trace-canvas--fullscreen" : "",
  ]
    .filter(Boolean)
    .join(" ");

  const header = (
    <div className="trace-canvas__header">
      <div>
        <h3 className="trace-canvas__title">Request path</h3>
        {trace && (
          <p className="trace-canvas__meta">
            {trace.nodes.length} step{trace.nodes.length === 1 ? "" : "s"}
            {trace.latency_ms != null
              ? ` · ${formatLatencyMs(trace.latency_ms)} total`
              : ""}
            {trace.truncated ? " · trace has more detail in Phoenix" : ""}
          </p>
        )}
      </div>
      {onToggleFullscreen && (
        <button
          type="button"
          className="btn btn--sm"
          onClick={onToggleFullscreen}
          aria-pressed={fullscreen}
        >
          {fullscreen ? "Exit fullscreen" : "Fullscreen"}
        </button>
      )}
    </div>
  );

  if (loading) {
    return (
      <div className={canvasClass} aria-busy="true">
        {header}
        <p className="panel__hint">Loading workflow…</p>
        <div className="trace-graph trace-graph--empty" aria-hidden="true" />
      </div>
    );
  }

  if (error) {
    return (
      <div className={canvasClass}>
        {header}
        <p className="panel__hint panel__hint--error" role="alert">
          {error}
        </p>
      </div>
    );
  }

  if (!trace || nodes.length === 0) {
    return (
      <div className={canvasClass}>
        {header}
        <p className="panel__hint">
          {sessionId
            ? "No workflow steps for this request."
            : "Select a request from the table to see its path."}
        </p>
      </div>
    );
  }

  return (
    <div className={canvasClass} aria-label="Request workflow diagram">
      {header}
      <p className="trace-canvas__legend">
        <span className="trace-legend trace-legend--ok">Success</span>
        <span className="trace-legend trace-legend--error">Failed</span>
      </p>
      <div className="trace-graph">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={workflowNodeTypes}
          fitView
          fitViewOptions={{ padding: 0.35 }}
          minZoom={0.15}
          maxZoom={1.25}
          nodesDraggable={false}
          nodesConnectable={false}
          elementsSelectable={false}
          panOnScroll
          proOptions={{ hideAttribution: true }}
        >
          <Background gap={20} color="var(--border)" />
          <Controls showInteractive={false} position="bottom-right" />
        </ReactFlow>
      </div>
    </div>
  );
}
