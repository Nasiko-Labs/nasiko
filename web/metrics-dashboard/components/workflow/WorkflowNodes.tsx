"use client";

import { Handle, Position, type NodeProps } from "@xyflow/react";
import { formatLatencyMs } from "@/lib/format";
import type { TraceGraphNode } from "@/lib/types";

type WorkflowNodeData = TraceGraphNode["data"];

function OpenAiIcon() {
  return (
    <svg
      className="wf-node__icon-svg"
      viewBox="0 0 24 24"
      aria-hidden="true"
      fill="currentColor"
    >
      <path d="M11.32 2.576a4.48 4.48 0 0 1 2.304.616 4.496 4.496 0 0 1 1.632 1.68l.048.096a4.48 4.48 0 0 1 .616 2.304 4.48 4.48 0 0 1-.616 2.304l-.048.096a4.496 4.496 0 0 1-1.68 1.632 4.48 4.48 0 0 1-2.304.616 4.48 4.48 0 0 1-2.304-.616 4.496 4.496 0 0 1-1.632-1.68l-.048-.096a4.48 4.48 0 0 1-.616-2.304 4.48 4.48 0 0 1 .616-2.304l.048-.096a4.496 4.496 0 0 1 1.68-1.632 4.48 4.48 0 0 1 2.304-.616zm.68 3.2a3.28 3.28 0 1 0 0 6.56 3.28 3.28 0 0 0 0-6.56zM6.4 11.52a3.28 3.28 0 1 0 0 6.56 3.28 3.28 0 0 0 0-6.56zm11.2 0a3.28 3.28 0 1 0 0 6.56 3.28 3.28 0 0 0 0-6.56zM12 17.6a3.28 3.28 0 1 0 0 6.56 3.28 3.28 0 0 0 0-6.56z" />
    </svg>
  );
}

function UserIcon() {
  return (
    <svg
      className="wf-node__icon-svg"
      viewBox="0 0 24 24"
      aria-hidden="true"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
    >
      <path d="M20 21a8 8 0 0 0-16 0" />
      <circle cx="12" cy="8" r="4" />
    </svg>
  );
}

function AgentIcon() {
  return (
    <svg
      className="wf-node__icon-svg"
      viewBox="0 0 24 24"
      aria-hidden="true"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
    >
      <rect x="4" y="4" width="16" height="16" rx="3" />
      <path d="M9 10h.01M15 10h.01M9.5 15a3 3 0 0 0 5 0" />
    </svg>
  );
}

function ToolIcon() {
  return (
    <svg
      className="wf-node__icon-svg"
      viewBox="0 0 24 24"
      aria-hidden="true"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
    >
      <path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L4 17l3 3 5.3-5.3a4 4 0 0 0 5.4-5.4l-2.6-2.6z" />
    </svg>
  );
}

function StepIcon() {
  return (
    <svg
      className="wf-node__icon-svg"
      viewBox="0 0 24 24"
      aria-hidden="true"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
    >
      <circle cx="12" cy="12" r="9" />
      <path d="M12 8v4l3 2" />
    </svg>
  );
}

function NodeIcon({ kind }: { kind: WorkflowNodeData["node_kind"] }) {
  switch (kind) {
    case "user":
      return <UserIcon />;
    case "llm":
      return <OpenAiIcon />;
    case "agent":
      return <AgentIcon />;
    case "tool":
      return <ToolIcon />;
    default:
      return <StepIcon />;
  }
}

export function WorkflowNodeCard({ data }: NodeProps) {
  const nodeData = data as WorkflowNodeData;
  const kind = nodeData.node_kind ?? "step";
  const tone = nodeData.tone ?? "neutral";

  return (
    <div className={`wf-node wf-node--${kind} wf-node--${tone}`}>
      <Handle type="target" position={Position.Left} className="wf-node__handle" />
      <Handle type="source" position={Position.Right} className="wf-node__handle" />
      <div className="wf-node__icon" aria-hidden="true">
        <NodeIcon kind={kind} />
      </div>
      <div className="wf-node__body">
        <p className="wf-node__title" title={nodeData.label}>
          {nodeData.label}
        </p>
        {nodeData.subtitle && (
          <p className="wf-node__subtitle">{nodeData.subtitle}</p>
        )}
        {nodeData.latency_ms != null && (
          <p className="wf-node__meta">{formatLatencyMs(nodeData.latency_ms)}</p>
        )}
      </div>
    </div>
  );
}

export const workflowNodeTypes = { workflow: WorkflowNodeCard };
